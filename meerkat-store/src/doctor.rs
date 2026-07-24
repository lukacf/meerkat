//! Read-only disk diagnosis behind `rkat storage doctor` (Phase 1 of the
//! storage unification arc).
//!
//! # Safety contract — safe against a live realm
//!
//! The first thing an operator does at 2 AM is run doctor against the wedged
//! production store, so this module:
//!
//! - never takes realm leases;
//! - never opens `Primary`-profile connections (those set pragmas and create
//!   files) — only [`meerkat_sqlite::ConnectionProfile::ReadOnly`] opens and
//!   raw `SELECT`s (the session-view queries grouped under one deferred read
//!   snapshot per database, so a live migration cannot hide a session
//!   between them);
//! - never creates files or directories;
//! - never runs the schema ledger (versions are read with
//!   [`meerkat_sqlite::domain_version`], nothing is applied);
//! - reads **only** the roots named in the [`DiagnoseScope`] — no ambient
//!   root resolution.
//!
//! # Fault tolerance
//!
//! Per-realm-entry: one corrupt manifest or database yields a finding for
//! that entry and never aborts the sweep (contrast
//! `list_realm_manifests_in`, which fails the whole listing on one corrupt
//! manifest).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use meerkat_core::storage_diagnostics::{
    DatabaseInventory, DiagnoseScope, FindingSeverity, StorageDiagnosis, StorageDiagnosticsError,
    StorageFinding, StorageInventoryEntry, StorageMigrator,
};
use meerkat_core::{
    BlobId, ContentBlock, Message, REALM_MANIFEST_FILE_NAME, Session,
    SessionCheckpointMetadataState, SessionId, SystemNoticeBlock, sanitize_realm_id,
    session_checkpoint_metadata_state,
};
use meerkat_sqlite::JsonColumnBytes;
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;

use crate::realm::{
    MANIFEST_LOCK_STALE_AFTER, REALM_LEASE_STALE_TTL_SECS, RealmLeaseRecord,
    SUPPORTED_MANIFEST_FORMAT,
};

// Stable kebab-case finding codes (shape-stable: never renamed).
/// Same realm id materialized under more than one swept root.
pub const FINDING_SPLIT_BRAIN_REALM: &str = "split-brain-realm";
/// A ledger domain version is newer than this binary supports.
pub const FINDING_SCHEMA_FROM_THE_FUTURE: &str = "schema-from-the-future";
/// Session documents without a typed checkpoint stamp.
pub const FINDING_LEGACY_UNVERIFIED_SESSIONS: &str = "legacy-unverified-sessions";
/// A session references a blob object missing from the realm's blob store.
pub const FINDING_DANGLING_BLOB_REFERENCE: &str = "dangling-blob-reference";
/// A lease record older than the staleness window.
pub const FINDING_ORPHANED_LEASE: &str = "orphaned-lease";
/// A live lease record (the realm is in use).
pub const FINDING_ACTIVE_LEASE: &str = "active-lease";
/// A lease file that does not parse (blocks destructive prune).
pub const FINDING_UNPARSEABLE_LEASE: &str = "unparseable-lease";
/// An existing database file with no migration ledger (pre-arc; expected).
pub const FINDING_NO_SCHEMA_LEDGER: &str = "no-schema-ledger";
/// A `*.pre-<version>-<timestamp>` migration backup artifact.
pub const FINDING_BACKUP_ARTIFACT: &str = "backup-artifact";
/// A `*.mfence` maintenance-fence lock file (inventory; created by normal
/// per-operation guards).
pub const FINDING_MAINTENANCE_FENCE_LOCK: &str = "maintenance-fence-lock";
/// A `.realm_manifest.lock` older than the 30s staleness window.
pub const FINDING_STALE_MANIFEST_LOCK: &str = "stale-manifest-lock";
/// A quarantined corrupt index (`*.corrupt-<timestamp>`).
pub const FINDING_QUARANTINED_INDEX: &str = "quarantined-index";
/// A realm manifest that cannot be read or parsed.
pub const FINDING_REALM_MANIFEST_UNREADABLE: &str = "realm-manifest-unreadable";
/// A database file that cannot be opened or queried read-only.
pub const FINDING_DATABASE_UNREADABLE: &str = "database-unreadable";
/// Checkpoint census skipped on a JSONL realm (index metadata is not
/// reliable evidence there).
pub const FINDING_CENSUS_SKIPPED_JSONL: &str = "census-skipped-jsonl";
/// Session checkpoint metadata that is present but malformed (never
/// laundered into "legacy").
pub const FINDING_CHECKPOINT_METADATA_INVALID: &str = "checkpoint-metadata-invalid";
/// Persisted session/message documents that do not decode (blob sweep;
/// error severity — an undecodable canonical document is one the runtime
/// cannot load either).
pub const FINDING_SESSION_DOCUMENT_UNDECODABLE: &str = "session-document-undecodable";
/// Internal doctor failure (the sweep task itself failed).
pub const FINDING_DOCTOR_INTERNAL: &str = "doctor-internal";
/// A realm manifest whose `manifest_format` is newer than this binary
/// understands. Normal startup refuses it typed; doctor reports it and does
/// not sweep the fixed disk layout a future format may have relocated.
pub const FINDING_MANIFEST_FROM_THE_FUTURE: &str = "manifest-from-the-future";
/// A realm pinned to an external storage provider; its storage is diagnosed
/// by that provider's migrator, never by the disk sweep.
pub const FINDING_EXTERNAL_PROVIDER_REALM: &str = "external-provider-realm";
/// A required storage path occupied by the wrong file type (directory,
/// FIFO, socket, broken symlink, ...): the artifact exists but must never
/// census as merely absent.
pub const FINDING_STORAGE_PATH_WRONG_TYPE: &str = "storage-path-wrong-type";
/// A ledger domain name outside this binary's domain registry
/// ([`KNOWN_LEDGER_DOMAINS`]) — likely stamped by a newer or foreign binary.
pub const FINDING_UNKNOWN_LEDGER_DOMAIN: &str = "unknown-ledger-domain";
/// A candidate realms root that exists but cannot be listed (permissions)
/// or is occupied by a non-directory: nothing under it was diagnosed, so a
/// clean report would be a lie.
pub const FINDING_STATE_ROOT_UNREADABLE: &str = "state-root-unreadable";
/// A cross-candidate first-start reservation marker
/// (`.realm-first-start.<sanitized>.lock`) in a candidate root. Recent
/// markers are normal first-start coordination; stale ones are crash
/// leftovers, removed by age-based takeover on the next first start.
pub const FINDING_FIRST_START_MARKER: &str = "first-start-marker";

/// Cap on individually reported dangling blob references per database; the
/// remainder is summarized in one finding so doctor stays usable on huge
/// realms.
const DANGLING_BLOB_REPORT_CAP: usize = 50;

/// Doctor's staleness horizon for first-start reservation markers: younger
/// markers are live coordination (info), older ones are crash leftovers
/// (warning). Deliberately far above the store's own takeover window so a
/// marker mid-takeover is never flagged.
const FIRST_START_MARKER_STALE_AFTER: Duration = Duration::from_secs(600);

const FIRST_START_MARKER_PREFIX: &str = ".realm-first-start.";
const FIRST_START_MARKER_SUFFIX: &str = ".lock";

/// Database files probed per realm directory, with the ledger domains their
/// owning stores stamp there. Shared with the Phase 6 migration framework
/// (`rkat storage migrate` reports the same file × domain matrix). Per-mob
/// databases (`mobs/<name>.db`, domain `mob`) are enumerated dynamically,
/// mirroring `enumerate_realm_sqlite_files` in `migrate.rs`.
pub const REALM_DATABASE_FILES: &[(&str, &[&str])] = &[
    (
        "sessions.sqlite3",
        &["session-store", "schedule-store", "runtime-store"],
    ),
    ("runtime.sqlite3", &["runtime-store"]),
    ("workgraph.sqlite3", &["workgraph"]),
    ("jobs.sqlite3", &["jobs"]),
    ("memory/memory.sqlite3", &["memory"]),
    ("tasks.db", &["tools-tasks"]),
    ("sessions_jsonl/session_index.sqlite3", &["jsonl-index"]),
];

/// Every ledger domain name a meerkat store stamps, across the whole crate
/// graph. Domains owned by crates above `meerkat-store` cannot have their
/// supported versions imported here without inverting the dependency order,
/// but their *names* are doctor vocabulary: a ledger row outside this
/// registry is reported as [`FINDING_UNKNOWN_LEDGER_DOMAIN`] instead of
/// being silently inventoried.
pub const KNOWN_LEDGER_DOMAINS: &[&str] = &[
    "session-store",
    "schedule-store",
    "runtime-store",
    "workgraph",
    "jobs",
    "memory",
    "mob",
    "tools-tasks",
    "jsonl-index",
];

// The supported-version registry is shared with the migration framework:
// one authority for "which domains this binary can judge, and up to what
// version" (see `migrate::supported_domain_version`).
use crate::migrate::supported_domain_version;

/// Read-only diagnosis over exactly the roots in `scope` (see the module
/// docs for the safety contract).
pub async fn diagnose_disk_roots(scope: &DiagnoseScope) -> StorageDiagnosis {
    let scope = scope.clone();
    match tokio::task::spawn_blocking(move || diagnose_blocking(&scope)).await {
        Ok(diagnosis) => diagnosis,
        Err(join_error) => {
            let mut diagnosis = StorageDiagnosis::default();
            diagnosis.findings.push(StorageFinding::new(
                FindingSeverity::Error,
                FINDING_DOCTOR_INTERNAL,
                format!("diagnosis sweep task failed: {join_error}"),
            ));
            diagnosis
        }
    }
}

/// The disk implementation of the [`StorageMigrator`] diagnose seam.
///
/// Deliberately a dumb unit struct delegating to [`diagnose_disk_roots`]:
/// the Phase 4 `RealmStorageProvider` returns it from `migrator()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskStorageMigrator;

#[async_trait]
impl StorageMigrator for DiskStorageMigrator {
    async fn diagnose(
        &self,
        scope: &DiagnoseScope,
    ) -> Result<StorageDiagnosis, StorageDiagnosticsError> {
        Ok(diagnose_disk_roots(scope).await)
    }
}

fn diagnose_blocking(scope: &DiagnoseScope) -> StorageDiagnosis {
    let mut diagnosis = StorageDiagnosis::default();

    // Dedup candidate roots by canonical identity while preserving order, so
    // two spellings of one directory neither double-report nor fabricate a
    // split-brain twin.
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut seen_roots: Vec<PathBuf> = Vec::new();
    for root in &scope.state_roots {
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        if seen_roots.contains(&canonical) {
            continue;
        }
        seen_roots.push(canonical);
        roots.push(root.clone());
    }

    // realm id -> (display dir, canonical dir) per materialization.
    let mut twin_map: BTreeMap<String, Vec<(PathBuf, PathBuf)>> = BTreeMap::new();

    for root in &roots {
        sweep_root(root, scope.realm.as_deref(), &mut diagnosis, &mut twin_map);
    }

    for (realm, locations) in &twin_map {
        let mut distinct: Vec<&(PathBuf, PathBuf)> = Vec::new();
        for location in locations {
            if !distinct.iter().any(|(_, canon)| canon == &location.1) {
                distinct.push(location);
            }
        }
        if distinct.len() > 1 {
            let paths = distinct
                .iter()
                .map(|(display, _)| display.display().to_string())
                .collect::<Vec<_>>()
                .join(" and ");
            diagnosis.findings.push(
                StorageFinding::new(
                    FindingSeverity::Error,
                    FINDING_SPLIT_BRAIN_REALM,
                    format!(
                        "realm '{realm}' is materialized under multiple state roots: {paths}; \
                         reconcile with `rkat storage migrate` (Phase 6) before writing through \
                         either copy"
                    ),
                )
                .with_path(distinct[0].0.clone())
                .with_realm(realm.clone()),
            );
        }
    }

    diagnosis
}

/// Doctor's lenient typed view of a persisted realm manifest.
///
/// Deliberately parsed doctor-side instead of through the store's
/// fail-closed pin parse (`realm::parse_manifest_pin_bytes`): that parse
/// refuses future formats, external pins (in the disk composition), and
/// backends this build's features exclude — all states doctor must *report*
/// without aborting. The format ceiling is still judged against the one
/// authoritative [`SUPPORTED_MANIFEST_FORMAT`].
#[derive(Debug, Deserialize)]
struct ManifestSummary {
    realm_id: String,
    backend: String,
    /// Format 1 predates the field and is never serialized (mirrors
    /// `realm::default_manifest_format`).
    #[serde(default = "manifest_format_v1")]
    manifest_format: u32,
    /// External storage-provider pin; pre-field manifests carry it only in
    /// the `external:<name>` backend string.
    #[serde(default)]
    provider: Option<String>,
}

fn manifest_format_v1() -> u32 {
    1
}

impl ManifestSummary {
    /// Explicit `provider` field, with the `external:<name>` backend-string
    /// fallback older external pins used (mirrors `realm.rs`).
    fn provider_name(&self) -> Option<&str> {
        self.provider
            .as_deref()
            .or_else(|| self.backend.strip_prefix("external:"))
    }
}

/// Why a manifest could not be read into a [`ManifestSummary`].
enum ManifestFault {
    /// Unreadable or unparseable content
    /// ([`FINDING_REALM_MANIFEST_UNREADABLE`]).
    Unreadable(String),
    /// The manifest path is occupied by the wrong file type
    /// ([`FINDING_STORAGE_PATH_WRONG_TYPE`]).
    WrongType(String),
}

fn read_manifest_summary(path: &Path) -> Result<ManifestSummary, String> {
    let bytes = std::fs::read(path).map_err(|err| format!("manifest unreadable: {err}"))?;
    serde_json::from_slice(&bytes).map_err(|err| format!("manifest does not parse: {err}"))
}

/// What actually occupies a required storage path. `is_file()` alone folds
/// a directory, FIFO, broken symlink, or failing metadata probe into
/// "absent" and lets a damaged realm produce a clean report; doctor keeps
/// the three states distinct.
enum PathProbe {
    Absent,
    File,
    /// Path exists but is not a regular file (description of what it is).
    WrongType(&'static str),
    /// Metadata probe failed for a reason other than absence.
    Unreadable(std::io::Error),
}

fn probe_required_file(path: &Path) -> PathProbe {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return PathProbe::Absent,
        Err(err) => return PathProbe::Unreadable(err),
    };
    if metadata.file_type().is_symlink() {
        // Follow the link: a symlink to a regular file is a valid layout.
        return match std::fs::metadata(path) {
            Ok(target) if target.is_file() => PathProbe::File,
            Ok(target) if target.is_dir() => PathProbe::WrongType("a symlink to a directory"),
            Ok(_) => PathProbe::WrongType("a symlink to a non-regular file"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                PathProbe::WrongType("a broken symlink")
            }
            Err(err) => PathProbe::Unreadable(err),
        };
    }
    if metadata.is_file() {
        PathProbe::File
    } else if metadata.is_dir() {
        PathProbe::WrongType("a directory")
    } else {
        PathProbe::WrongType("a non-regular file (fifo/socket/device)")
    }
}

fn sweep_root(
    root: &Path,
    realm_filter: Option<&str>,
    diagnosis: &mut StorageDiagnosis,
    twin_map: &mut BTreeMap<String, Vec<(PathBuf, PathBuf)>>,
) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        // An absent candidate root is a normal state, not a finding.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        // An unreadable or wrong-typed root must never fold into a clean
        // report: nothing under it was diagnosed.
        Err(err) => {
            let (code, message) = match std::fs::metadata(root) {
                Ok(metadata) if !metadata.is_dir() => (
                    FINDING_STORAGE_PATH_WRONG_TYPE,
                    format!("realms root is not a directory: {err}"),
                ),
                _ => (
                    FINDING_STATE_ROOT_UNREADABLE,
                    format!("cannot list realms root: {err}"),
                ),
            };
            diagnosis.findings.push(
                StorageFinding::new(FindingSeverity::Error, code, message)
                    .with_path(root.to_path_buf()),
            );
            return;
        }
    };
    let mut realm_dirs: Vec<PathBuf> = Vec::new();
    let mut first_start_markers: Vec<PathBuf> = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            realm_dirs.push(path);
        } else if first_start_marker_slug(&path).is_some() {
            first_start_markers.push(path);
        }
    }
    realm_dirs.sort();
    first_start_markers.sort();
    sweep_first_start_markers(&first_start_markers, realm_filter, diagnosis);

    for dir in realm_dirs {
        let dir_name = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if crate::migrate::is_backup_artifact_name(&dir_name) {
            // An archived realm copy under the registered backup naming
            // (`rkat storage migrate` split-brain resolution). It still
            // carries a manifest, but it is a frozen artifact, not a live
            // realm — treating it as one would resurrect the split-brain
            // finding forever. `rkat storage prune` owns its lifecycle.
            diagnosis.findings.push(
                StorageFinding::new(
                    FindingSeverity::Info,
                    FINDING_BACKUP_ARTIFACT,
                    "archived realm directory (`*.pre-<version>-<timestamp>` backup artifact); \
                     lifecycle owned by `rkat storage prune`",
                )
                .with_path(dir.clone()),
            );
            continue;
        }
        let manifest_path = dir.join(REALM_MANIFEST_FILE_NAME);
        let manifest: Result<ManifestSummary, ManifestFault> =
            match probe_required_file(&manifest_path) {
                // No manifest at all: not a materialized realm directory.
                PathProbe::Absent => continue,
                PathProbe::File => {
                    read_manifest_summary(&manifest_path).map_err(ManifestFault::Unreadable)
                }
                PathProbe::WrongType(kind) => Err(ManifestFault::WrongType(format!(
                    "manifest path is {kind}, not a regular file"
                ))),
                PathProbe::Unreadable(err) => Err(ManifestFault::Unreadable(format!(
                    "manifest metadata unreadable: {err}"
                ))),
            };
        let (realm_label, backend) = match &manifest {
            Ok(summary) => (summary.realm_id.clone(), Some(summary.backend.clone())),
            Err(_) => (dir_name.clone(), None),
        };
        if let Some(filter) = realm_filter {
            let matches_dir = dir_name == sanitize_realm_id(filter);
            let matches_identity = realm_label == filter;
            if !matches_dir && !matches_identity {
                continue;
            }
        }
        match &manifest {
            Err(ManifestFault::Unreadable(detail)) => {
                diagnosis.findings.push(
                    StorageFinding::new(
                        FindingSeverity::Error,
                        FINDING_REALM_MANIFEST_UNREADABLE,
                        detail.clone(),
                    )
                    .with_path(manifest_path.clone())
                    .with_realm(realm_label.clone()),
                );
            }
            Err(ManifestFault::WrongType(detail)) => {
                diagnosis.findings.push(
                    StorageFinding::new(
                        FindingSeverity::Error,
                        FINDING_STORAGE_PATH_WRONG_TYPE,
                        detail.clone(),
                    )
                    .with_path(manifest_path.clone())
                    .with_realm(realm_label.clone()),
                );
            }
            Ok(_) => {}
        }
        let canonical_dir = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        twin_map
            .entry(realm_label.clone())
            .or_default()
            .push((dir.clone(), canonical_dir));

        let mut entry = StorageInventoryEntry::new(realm_label.clone(), dir.clone());
        entry.backend = backend.clone();
        match &manifest {
            // A future manifest format may have relocated storage; sweeping
            // the fixed disk layout would diagnose the wrong files while
            // normal startup correctly refuses the realm typed.
            Ok(summary) if summary.manifest_format > SUPPORTED_MANIFEST_FORMAT => {
                diagnosis.findings.push(
                    StorageFinding::new(
                        FindingSeverity::Error,
                        FINDING_MANIFEST_FROM_THE_FUTURE,
                        format!(
                            "realm manifest format {} is newer than the supported \
                             {SUPPORTED_MANIFEST_FORMAT}; diagnose with the newer binary — the \
                             fixed disk layout is not swept",
                            summary.manifest_format
                        ),
                    )
                    .with_path(manifest_path.clone())
                    .with_realm(realm_label.clone()),
                );
            }
            Ok(summary) => {
                if let Some(provider) = summary.provider_name() {
                    // Storage lives with the external provider; the disk
                    // layout under this directory is not the realm's data.
                    diagnosis.findings.push(
                        StorageFinding::new(
                            FindingSeverity::Info,
                            FINDING_EXTERNAL_PROVIDER_REALM,
                            format!(
                                "realm is pinned to external storage provider '{provider}'; \
                                 diagnosis belongs to that provider's migrator, not the disk \
                                 sweep"
                            ),
                        )
                        .with_path(manifest_path.clone())
                        .with_realm(realm_label.clone()),
                    );
                } else {
                    diagnose_realm_dir(
                        &dir,
                        &realm_label,
                        backend.as_deref(),
                        &mut entry,
                        diagnosis,
                    );
                }
            }
            // Unreadable/wrong-typed manifest (already a finding above): the
            // on-disk data is still real; diagnose it.
            Err(_) => {
                diagnose_realm_dir(&dir, &realm_label, None, &mut entry, diagnosis);
            }
        }
        diagnosis.inventory.push(entry);
    }
}

/// The sanitized realm slug of a first-start reservation marker file name
/// (`.realm-first-start.<sanitized>.lock`), `None` for anything else.
fn first_start_marker_slug(path: &Path) -> Option<&str> {
    path.file_name()?
        .to_str()?
        .strip_prefix(FIRST_START_MARKER_PREFIX)?
        .strip_suffix(FIRST_START_MARKER_SUFFIX)
        .filter(|slug| !slug.is_empty())
}

/// Doctor's lenient view of the marker payload (`realm.rs` writes
/// `{realm_id, pid, created_at_unix}`); a torn write falls back to mtime.
#[derive(Debug, Deserialize)]
struct FirstStartMarkerSummary {
    #[serde(default)]
    realm_id: Option<String>,
    #[serde(default)]
    created_at_unix: Option<u64>,
}

/// First-start reservation census over one candidate root: a recent marker
/// is normal cross-root first-start coordination; a stale one is a crash
/// leftover that the next first start of the realm removes by age-based
/// takeover.
fn sweep_first_start_markers(
    markers: &[PathBuf],
    realm_filter: Option<&str>,
    diagnosis: &mut StorageDiagnosis,
) {
    for path in markers {
        let Some(slug) = first_start_marker_slug(path) else {
            continue;
        };
        let payload = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<FirstStartMarkerSummary>(&bytes).ok());
        let realm_label = payload
            .as_ref()
            .and_then(|marker| marker.realm_id.clone())
            .unwrap_or_else(|| slug.to_string());
        if let Some(filter) = realm_filter
            && slug != sanitize_realm_id(filter)
            && realm_label != filter
        {
            continue;
        }
        // Payload timestamp first, mtime as the torn-write fallback
        // (mirrors the store's own takeover check). Unknown age reports
        // stale: freshness that cannot be certified is not assumed.
        let age = payload
            .as_ref()
            .and_then(|marker| marker.created_at_unix)
            .map(|created| Duration::from_secs(now_unix_secs().saturating_sub(created)))
            .or_else(|| {
                std::fs::metadata(path)
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            });
        let finding = match age {
            Some(age) if age <= FIRST_START_MARKER_STALE_AFTER => StorageFinding::new(
                FindingSeverity::Info,
                FINDING_FIRST_START_MARKER,
                "recent first-start reservation marker (a realm first start is in flight or \
                 just completed)",
            ),
            _ => StorageFinding::new(
                FindingSeverity::Warning,
                FINDING_FIRST_START_MARKER,
                format!(
                    "stale first-start reservation marker (older than {}s; the holder likely \
                     crashed mid-first-start); the next first start of this realm removes it \
                     by age-based takeover",
                    FIRST_START_MARKER_STALE_AFTER.as_secs()
                ),
            ),
        };
        diagnosis
            .findings
            .push(finding.with_path(path.clone()).with_realm(realm_label));
    }
}

/// Probe a candidate database path; absent is normal, wrong-typed or
/// unprobeable paths are findings. Returns whether a regular file is there.
fn probe_database_file(db_path: &Path, realm: &str, diagnosis: &mut StorageDiagnosis) -> bool {
    match probe_required_file(db_path) {
        PathProbe::File => true,
        PathProbe::Absent => false,
        PathProbe::WrongType(kind) => {
            diagnosis.findings.push(
                StorageFinding::new(
                    FindingSeverity::Error,
                    FINDING_STORAGE_PATH_WRONG_TYPE,
                    format!("database path is {kind}, not a regular file"),
                )
                .with_path(db_path.to_path_buf())
                .with_realm(realm),
            );
            false
        }
        PathProbe::Unreadable(err) => {
            diagnosis.findings.push(
                StorageFinding::new(
                    FindingSeverity::Error,
                    FINDING_DATABASE_UNREADABLE,
                    format!("cannot probe database file metadata: {err}"),
                )
                .with_path(db_path.to_path_buf())
                .with_realm(realm),
            );
            false
        }
    }
}

fn diagnose_realm_dir(
    realm_dir: &Path,
    realm: &str,
    backend: Option<&str>,
    entry: &mut StorageInventoryEntry,
    diagnosis: &mut StorageDiagnosis,
) {
    let mut sessions_db_swept = false;
    for (relative, expected_domains) in REALM_DATABASE_FILES {
        let db_path = realm_dir.join(relative);
        if !probe_database_file(&db_path, realm, diagnosis) {
            continue;
        }
        if *relative == "sessions.sqlite3" {
            sessions_db_swept = true;
        }
        entry.databases.push(inspect_database(
            &db_path,
            expected_domains,
            realm,
            diagnosis,
        ));
    }

    // Per-mob databases are enumerated dynamically (`mobs/<name>.db`),
    // mirroring `enumerate_realm_sqlite_files` in migrate.rs; each stamps
    // the `mob` ledger domain.
    if let Ok(dir_entries) = std::fs::read_dir(realm_dir.join("mobs")) {
        let mut mob_dbs: Vec<PathBuf> = dir_entries
            .filter_map(Result::ok)
            .map(|dir_entry| dir_entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("db"))
            .collect();
        mob_dbs.sort();
        for db_path in mob_dbs {
            if !probe_database_file(&db_path, realm, diagnosis) {
                continue;
            }
            entry
                .databases
                .push(inspect_database(&db_path, &["mob"], realm, diagnosis));
        }
    }

    match backend {
        Some("sqlite") => {
            let sessions_db = realm_dir.join("sessions.sqlite3");
            if sessions_db_swept {
                // No schema preflight: doctor must open future files to
                // report them (module safety contract).
                match meerkat_sqlite::open(
                    &sessions_db,
                    meerkat_sqlite::ConnectionProfile::ReadOnly,
                ) {
                    Ok(conn) => {
                        // One deferred read transaction so the checkpoint
                        // census and the blob sweep observe a single SQLite
                        // snapshot: a live legacy-to-strand migration landing
                        // between separate autocommit queries could otherwise
                        // move a session out of both views. The first SELECT
                        // inside the transaction establishes the snapshot.
                        match conn.unchecked_transaction() {
                            Ok(tx) => {
                                census_checkpoint_evidence(&tx, &sessions_db, realm, diagnosis);
                                sweep_dangling_blobs(
                                    &tx,
                                    realm_dir,
                                    &sessions_db,
                                    realm,
                                    diagnosis,
                                );
                            }
                            Err(err) => {
                                diagnosis.findings.push(
                                    StorageFinding::new(
                                        FindingSeverity::Error,
                                        FINDING_DATABASE_UNREADABLE,
                                        format!("cannot begin read-snapshot transaction: {err}"),
                                    )
                                    .with_path(sessions_db.clone())
                                    .with_realm(realm),
                                );
                            }
                        }
                    }
                    Err(_) => {
                        // Already reported by inspect_database above.
                    }
                }
            }
        }
        Some("jsonl") => {
            diagnosis.findings.push(
                StorageFinding::new(
                    FindingSeverity::Info,
                    FINDING_CENSUS_SKIPPED_JSONL,
                    "checkpoint-evidence census skipped: JSONL index metadata is not reliable \
                     evidence (pre-metadata index rows census as unstamped)",
                )
                .with_path(realm_dir.join("sessions_jsonl"))
                .with_realm(realm),
            );
        }
        _ => {}
    }

    sweep_artifacts(realm_dir, realm, diagnosis);
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, rusqlite::Error> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn inspect_database(
    db_path: &Path,
    expected_domains: &[&str],
    realm: &str,
    diagnosis: &mut StorageDiagnosis,
) -> DatabaseInventory {
    let mut inventory = DatabaseInventory::new(db_path.to_path_buf());
    // No schema preflight: inspecting (and reporting) future-versioned
    // ledgers is this function's job.
    let conn = match meerkat_sqlite::open(db_path, meerkat_sqlite::ConnectionProfile::ReadOnly) {
        Ok(conn) => conn,
        Err(err) => {
            diagnosis.findings.push(
                StorageFinding::new(
                    FindingSeverity::Error,
                    FINDING_DATABASE_UNREADABLE,
                    format!("cannot open database read-only: {err}"),
                )
                .with_path(db_path.to_path_buf())
                .with_realm(realm),
            );
            return inventory;
        }
    };

    match read_ledger_rows(&conn) {
        Ok(Some(rows)) => {
            for (domain, version) in &rows {
                match supported_domain_version(domain) {
                    Some(supported) if *version > supported => {
                        diagnosis.findings.push(
                            StorageFinding::new(
                                FindingSeverity::Error,
                                FINDING_SCHEMA_FROM_THE_FUTURE,
                                format!(
                                    "ledger domain '{domain}' is at version {version} but this \
                                     binary supports at most {supported}; refuse to open with an \
                                     older binary (rollback candidate fails certification)"
                                ),
                            )
                            .with_path(db_path.to_path_buf())
                            .with_realm(realm),
                        );
                    }
                    Some(_) => {}
                    // Known domain owned above this crate in the dependency
                    // order: inventoried below, version judged only by the
                    // owning store.
                    None if KNOWN_LEDGER_DOMAINS.contains(&domain.as_str()) => {}
                    None => {
                        diagnosis.findings.push(
                            StorageFinding::new(
                                FindingSeverity::Warning,
                                FINDING_UNKNOWN_LEDGER_DOMAIN,
                                format!(
                                    "ledger domain '{domain}' (version {version}) is not in this \
                                     binary's domain registry — likely stamped by a newer or \
                                     foreign binary; its schema version cannot be certified here"
                                ),
                            )
                            .with_path(db_path.to_path_buf())
                            .with_realm(realm),
                        );
                    }
                }
                inventory.domains.push((domain.clone(), Some(*version)));
            }
            for expected in expected_domains {
                if !rows.iter().any(|(domain, _)| domain == expected) {
                    inventory.domains.push(((*expected).to_string(), None));
                }
            }
        }
        Ok(None) => {
            diagnosis.findings.push(
                StorageFinding::new(
                    FindingSeverity::Info,
                    FINDING_NO_SCHEMA_LEDGER,
                    "existing database has no meerkat_schema ledger (written before the \
                     migration-ledger arc; expected — the owning store baselines it on next \
                     write open)",
                )
                .with_path(db_path.to_path_buf())
                .with_realm(realm),
            );
            for expected in expected_domains {
                inventory.domains.push(((*expected).to_string(), None));
            }
        }
        Err(err) => {
            diagnosis.findings.push(
                StorageFinding::new(
                    FindingSeverity::Error,
                    FINDING_DATABASE_UNREADABLE,
                    format!("cannot read schema ledger: {err}"),
                )
                .with_path(db_path.to_path_buf())
                .with_realm(realm),
            );
        }
    }
    inventory
}

/// `Ok(None)` = no ledger table; `Ok(Some(rows))` = every ledger row.
fn read_ledger_rows(conn: &Connection) -> Result<Option<Vec<(String, i64)>>, rusqlite::Error> {
    if !table_exists(conn, "meerkat_schema")? {
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

/// Checkpoint-evidence census over the sqlite session store: raw read-only
/// SQL over `session_heads.metadata_json` (canonical representation) plus
/// `sessions.metadata_json` for sessions without a head row, evaluated with
/// the core metadata census helper. Callers pass a connection holding the
/// per-database read snapshot (see `diagnose_realm_dir`) so the two queries
/// see one consistent view.
fn census_checkpoint_evidence(
    conn: &Connection,
    db_path: &Path,
    realm: &str,
    diagnosis: &mut StorageDiagnosis,
) {
    let mut verified = 0usize;
    let mut legacy = 0usize;
    let mut invalid = 0usize;

    let mut classify = |session_id: &str, metadata_json: &[u8]| {
        let Ok(id) = SessionId::parse(session_id) else {
            invalid += 1;
            return;
        };
        let Ok(metadata) =
            serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(metadata_json)
        else {
            invalid += 1;
            return;
        };
        match session_checkpoint_metadata_state(&id, &metadata) {
            Ok(SessionCheckpointMetadataState::Stamped(_)) => verified += 1,
            Ok(SessionCheckpointMetadataState::LegacyUnverified { .. }) => legacy += 1,
            Err(_) => invalid += 1,
        }
    };

    let result = (|| -> Result<(), rusqlite::Error> {
        let heads_exist = table_exists(conn, "session_heads")?;
        let sessions_exist = table_exists(conn, "sessions")?;
        if heads_exist {
            let mut statement = conn.prepare(
                "SELECT session_id, metadata_json FROM session_heads ORDER BY session_id",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let session_id: String = row.get(0)?;
                let metadata_json: JsonColumnBytes = row.get(1)?;
                classify(&session_id, &metadata_json.into_bytes());
            }
        }
        if sessions_exist {
            // A head row makes the head representation canonical; the blob
            // row is then a frozen migration archive and not census evidence.
            let sql = if heads_exist {
                "SELECT session_id, metadata_json FROM sessions \
                 WHERE session_id NOT IN (SELECT session_id FROM session_heads) \
                 ORDER BY session_id"
            } else {
                "SELECT session_id, metadata_json FROM sessions ORDER BY session_id"
            };
            let mut statement = conn.prepare(sql)?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let session_id: String = row.get(0)?;
                let metadata_json: JsonColumnBytes = row.get(1)?;
                classify(&session_id, &metadata_json.into_bytes());
            }
        }
        Ok(())
    })();

    if let Err(err) = result {
        diagnosis.findings.push(
            StorageFinding::new(
                FindingSeverity::Error,
                FINDING_DATABASE_UNREADABLE,
                format!("checkpoint census query failed: {err}"),
            )
            .with_path(db_path.to_path_buf())
            .with_realm(realm),
        );
        return;
    }

    if legacy > 0 {
        diagnosis.findings.push(
            StorageFinding::new(
                FindingSeverity::Warning,
                FINDING_LEGACY_UNVERIFIED_SESSIONS,
                format!(
                    "{legacy} legacy-unverified session document(s) ({verified} verified); \
                     resume auto-migrates each on first touch, bulk adoption arrives with \
                     `rkat storage migrate`"
                ),
            )
            .with_path(db_path.to_path_buf())
            .with_realm(realm),
        );
    }
    if invalid > 0 {
        diagnosis.findings.push(
            StorageFinding::new(
                FindingSeverity::Error,
                FINDING_CHECKPOINT_METADATA_INVALID,
                format!(
                    "{invalid} session document(s) carry malformed checkpoint metadata \
                     (present-but-invalid evidence is never laundered into legacy)"
                ),
            )
            .with_path(db_path.to_path_buf())
            .with_realm(realm),
        );
    }
}

/// The on-disk object path `FsBlobStore` uses for a canonical blob id:
/// `<blobs>/<first-2-hex>/<hex>.json`.
fn blob_object_path(blobs_root: &Path, blob_id: &BlobId) -> Option<PathBuf> {
    if !blob_id.is_canonical_sha256() {
        return None;
    }
    let key = blob_id.as_str().strip_prefix("sha256:")?;
    let prefix = key.get(0..2).unwrap_or("xx");
    Some(blobs_root.join(prefix).join(format!("{key}.json")))
}

fn collect_content_block_blob_refs(blocks: &[ContentBlock], refs: &mut Vec<BlobId>) {
    for block in blocks {
        if let Some((_, blob_id)) = block.image_blob_ref() {
            refs.push(blob_id.clone());
        }
    }
}

fn collect_message_blob_refs(message: &Message, refs: &mut Vec<BlobId>) {
    match message {
        Message::User(user) => collect_content_block_blob_refs(&user.content, refs),
        Message::ToolResults { results, .. } => {
            for result in results {
                collect_content_block_blob_refs(&result.content, refs);
            }
        }
        Message::SystemNotice(notice) => {
            for block in &notice.blocks {
                match block {
                    SystemNoticeBlock::Comms { content, .. }
                    | SystemNoticeBlock::ExternalEvent { content, .. } => {
                        collect_content_block_blob_refs(content, refs);
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Per-database accounting for dangling blob references.
///
/// `seen` makes duplicate checks O(1) per reference; collection into
/// `reported` stops at [`DANGLING_BLOB_REPORT_CAP`] and the remainder is
/// only counted, so a hugely damaged realm costs linear time and bounded
/// report memory instead of a quadratic scan over an unbounded list.
struct DanglingCollector {
    /// blob id → object file exists (each blob probed on disk once).
    existence: HashMap<String, bool>,
    /// Distinct (session id, blob id) pairs already accounted.
    seen: HashSet<(String, String)>,
    reported: Vec<(String, BlobId)>,
    overflow: usize,
}

impl DanglingCollector {
    fn new() -> Self {
        Self {
            existence: HashMap::new(),
            seen: HashSet::new(),
            reported: Vec::new(),
            overflow: 0,
        }
    }

    fn record(&mut self, blobs_root: &Path, session_id: &str, refs: Vec<BlobId>) {
        for blob_id in refs {
            let exists = *self
                .existence
                .entry(blob_id.as_str().to_string())
                .or_insert_with(|| {
                    blob_object_path(blobs_root, &blob_id).is_some_and(|path| path.is_file())
                });
            if exists {
                continue;
            }
            let key = (session_id.to_string(), blob_id.as_str().to_string());
            if !self.seen.insert(key) {
                continue;
            }
            if self.reported.len() < DANGLING_BLOB_REPORT_CAP {
                self.reported.push((session_id.to_string(), blob_id));
            } else {
                self.overflow += 1;
            }
        }
    }

    fn total(&self) -> usize {
        self.reported.len() + self.overflow
    }
}

/// Dangling session→blob reference sweep (sqlite backend): decode persisted
/// session documents and strand messages, walk them for
/// `ImageData::Blob { blob_id }`, and probe the realm's `blobs/` directory
/// for each referenced object. Callers pass a connection holding the
/// per-database read snapshot (see `diagnose_realm_dir`).
fn sweep_dangling_blobs(
    conn: &Connection,
    realm_dir: &Path,
    db_path: &Path,
    realm: &str,
    diagnosis: &mut StorageDiagnosis,
) {
    let blobs_root = realm_dir.join("blobs");
    let mut collector = DanglingCollector::new();
    let mut undecodable = 0usize;

    let result = (|| -> Result<(), rusqlite::Error> {
        let heads_exist = table_exists(conn, "session_heads")?;
        if table_exists(conn, "session_strand_messages")? {
            let mut statement = conn.prepare(
                "SELECT session_id, message_json FROM session_strand_messages \
                 ORDER BY session_id, strand, seq",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let session_id: String = row.get(0)?;
                let message_json: JsonColumnBytes = row.get(1)?;
                match serde_json::from_slice::<Message>(&message_json.into_bytes()) {
                    Ok(message) => {
                        let mut refs = Vec::new();
                        collect_message_blob_refs(&message, &mut refs);
                        collector.record(&blobs_root, &session_id, refs);
                    }
                    Err(_) => undecodable += 1,
                }
            }
        }
        if table_exists(conn, "sessions")? {
            // Sessions with a head row keep their blob row only as a frozen
            // migration archive; their live transcript is the strand rows
            // already swept above.
            let sql = if heads_exist {
                "SELECT session_id, session_json FROM sessions \
                 WHERE session_id NOT IN (SELECT session_id FROM session_heads) \
                 ORDER BY session_id"
            } else {
                "SELECT session_id, session_json FROM sessions ORDER BY session_id"
            };
            let mut statement = conn.prepare(sql)?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let session_id: String = row.get(0)?;
                let session_json: JsonColumnBytes = row.get(1)?;
                match serde_json::from_slice::<Session>(&session_json.into_bytes()) {
                    Ok(session) => {
                        let mut refs = Vec::new();
                        for message in session.messages() {
                            collect_message_blob_refs(message, &mut refs);
                        }
                        collector.record(&blobs_root, &session_id, refs);
                    }
                    Err(_) => undecodable += 1,
                }
            }
        }
        Ok(())
    })();

    if let Err(err) = result {
        diagnosis.findings.push(
            StorageFinding::new(
                FindingSeverity::Error,
                FINDING_DATABASE_UNREADABLE,
                format!("dangling-blob sweep query failed: {err}"),
            )
            .with_path(db_path.to_path_buf())
            .with_realm(realm),
        );
        return;
    }

    let total = collector.total();
    for (session_id, blob_id) in &collector.reported {
        let mut finding = StorageFinding::new(
            FindingSeverity::Error,
            FINDING_DANGLING_BLOB_REFERENCE,
            format!("session {session_id} references missing blob {blob_id}"),
        )
        .with_realm(realm);
        if let Some(expected) = blob_object_path(&blobs_root, blob_id) {
            finding = finding.with_path(expected);
        } else {
            finding = finding.with_path(db_path.to_path_buf());
        }
        diagnosis.findings.push(finding);
    }
    if collector.overflow > 0 {
        diagnosis.findings.push(
            StorageFinding::new(
                FindingSeverity::Error,
                FINDING_DANGLING_BLOB_REFERENCE,
                format!(
                    "{} additional dangling blob reference(s) not listed individually \
                     ({total} total)",
                    collector.overflow
                ),
            )
            .with_path(db_path.to_path_buf())
            .with_realm(realm),
        );
    }
    if undecodable > 0 {
        // Error severity: these are canonical representations (strand rows,
        // or blob rows with no head); a document doctor cannot decode is one
        // the runtime cannot load either.
        diagnosis.findings.push(
            StorageFinding::new(
                FindingSeverity::Error,
                FINDING_SESSION_DOCUMENT_UNDECODABLE,
                format!(
                    "{undecodable} persisted session/message document(s) did not decode during \
                     the blob-reference sweep"
                ),
            )
            .with_path(db_path.to_path_buf())
            .with_realm(realm),
        );
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Orphan/artifact sweep: stale manifest locks, lease census, `*.mfence`
/// fence locks, `*.pre-<version>-<timestamp>` backup artifacts, and
/// `*.corrupt-<timestamp>` index quarantines.
fn sweep_artifacts(realm_dir: &Path, realm: &str, diagnosis: &mut StorageDiagnosis) {
    // Stale `.realm_manifest.lock` (creation lock; 30s mtime staleness
    // window, mirroring the store's own staleness rule).
    let manifest_lock = realm_dir.join(".realm_manifest.lock");
    if let Ok(metadata) = std::fs::metadata(&manifest_lock)
        && let Ok(modified) = metadata.modified()
        && SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO)
            > MANIFEST_LOCK_STALE_AFTER
    {
        diagnosis.findings.push(
            StorageFinding::new(
                FindingSeverity::Warning,
                FINDING_STALE_MANIFEST_LOCK,
                format!(
                    "manifest creation lock is older than the {}s staleness window (holder \
                     likely died; the store treats it as stale and removes it on next contention)",
                    MANIFEST_LOCK_STALE_AFTER.as_secs()
                ),
            )
            .with_path(manifest_lock)
            .with_realm(realm),
        );
    }

    // Lease census: reads only, mirrors the store's staleness rule; a lease
    // that fails serde parse is malformed/unknown-state, not proof of
    // absence (it blocks destructive prune).
    let lease_dir = realm_dir.join("leases");
    if let Ok(entries) = std::fs::read_dir(&lease_dir) {
        let now = now_unix_secs();
        let mut active = 0usize;
        let mut stale = 0usize;
        let mut unparseable = 0usize;
        let mut surfaces: Vec<String> = Vec::new();
        // Sorted so the surfaces listed in the active-lease message are
        // stable across runs (read_dir order is not).
        let mut lease_files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        lease_files.sort();
        for path in lease_files {
            match std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<RealmLeaseRecord>(&bytes).ok())
            {
                Some(record) => {
                    if now.saturating_sub(record.heartbeat_at) <= REALM_LEASE_STALE_TTL_SECS {
                        active += 1;
                        surfaces.push(format!("{} (pid {})", record.surface, record.pid));
                    } else {
                        stale += 1;
                    }
                }
                None => unparseable += 1,
            }
        }
        if active > 0 {
            diagnosis.findings.push(
                StorageFinding::new(
                    FindingSeverity::Info,
                    FINDING_ACTIVE_LEASE,
                    format!(
                        "{active} live realm lease(s): {} — the realm is in use (note: plain \
                         `rkat run` holds no lease, so absence of leases is not proof of no \
                         writer)",
                        surfaces.join(", ")
                    ),
                )
                .with_path(lease_dir.clone())
                .with_realm(realm),
            );
        }
        if stale > 0 {
            diagnosis.findings.push(
                StorageFinding::new(
                    FindingSeverity::Warning,
                    FINDING_ORPHANED_LEASE,
                    format!(
                        "{stale} stale lease file(s) older than the {REALM_LEASE_STALE_TTL_SECS}s \
                         heartbeat window (holder likely died)"
                    ),
                )
                .with_path(lease_dir.clone())
                .with_realm(realm),
            );
        }
        if unparseable > 0 {
            diagnosis.findings.push(
                StorageFinding::new(
                    FindingSeverity::Warning,
                    FINDING_UNPARSEABLE_LEASE,
                    format!(
                        "{unparseable} unparseable lease file(s); unknown liveness blocks \
                         destructive prune until removed by an operator"
                    ),
                )
                .with_path(lease_dir)
                .with_realm(realm),
            );
        }
    }

    // Filesystem artifacts next to the databases (one level: realm root plus
    // the known database subdirectories).
    let scan_dirs = [
        realm_dir.to_path_buf(),
        realm_dir.join("memory"),
        realm_dir.join("sessions_jsonl"),
    ];
    for scan_dir in &scan_dirs {
        let Ok(entries) = std::fs::read_dir(scan_dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        for file in files {
            let Some(name) = file.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.ends_with(".mfence") {
                diagnosis.findings.push(
                    StorageFinding::new(
                        FindingSeverity::Info,
                        FINDING_MAINTENANCE_FENCE_LOCK,
                        "maintenance-fence lock file (created by normal per-operation guards; \
                         held exclusively only during offline maintenance)",
                    )
                    .with_path(file.clone())
                    .with_realm(realm),
                );
            } else if name.contains(".pre-") {
                diagnosis.findings.push(
                    StorageFinding::new(
                        FindingSeverity::Info,
                        FINDING_BACKUP_ARTIFACT,
                        "migration backup artifact (`*.pre-<version>-<timestamp>`); lifecycle \
                         owned by `rkat storage prune` (Phase 6)",
                    )
                    .with_path(file.clone())
                    .with_realm(realm),
                );
            } else if name.contains(".corrupt-") {
                diagnosis.findings.push(
                    StorageFinding::new(
                        FindingSeverity::Warning,
                        FINDING_QUARANTINED_INDEX,
                        "quarantined corrupt index file (the store rebuilt a replacement; the \
                         quarantine is kept for inspection)",
                    )
                    .with_path(file.clone())
                    .with_realm(realm),
                );
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::{ImageData, UserMessage};

    fn write_manifest(realms_root: &Path, realm_id: &str, backend: &str) -> PathBuf {
        let dir = realms_root.join(sanitize_realm_id(realm_id));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(REALM_MANIFEST_FILE_NAME),
            serde_json::to_vec_pretty(&serde_json::json!({
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

    fn scope(roots: &[&Path]) -> DiagnoseScope {
        DiagnoseScope::new(roots.iter().map(|r| r.to_path_buf()).collect())
    }

    fn codes(diagnosis: &StorageDiagnosis) -> Vec<&str> {
        diagnosis.findings.iter().map(|f| f.code.as_str()).collect()
    }

    const SESSIONS_DDL: &str = "CREATE TABLE sessions (
        session_id TEXT PRIMARY KEY,
        created_at_ms INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL,
        message_count INTEGER NOT NULL,
        total_tokens INTEGER NOT NULL,
        metadata_json TEXT NOT NULL,
        session_json BLOB NOT NULL
    )";

    fn insert_session(conn: &Connection, session: &Session) {
        conn.execute(
            "INSERT INTO sessions (session_id, created_at_ms, updated_at_ms, message_count, \
             total_tokens, metadata_json, session_json) VALUES (?1, 0, 0, ?2, 0, ?3, ?4)",
            rusqlite::params![
                session.id().to_string(),
                session.messages().len() as i64,
                serde_json::to_string(session.metadata()).unwrap(),
                serde_json::to_vec(session).unwrap(),
            ],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn sweep_tolerates_corrupt_manifest_and_inventories_the_rest() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        write_manifest(&root, "healthy", "sqlite");
        let corrupt_dir = root.join("corrupt");
        std::fs::create_dir_all(&corrupt_dir).unwrap();
        std::fs::write(corrupt_dir.join(REALM_MANIFEST_FILE_NAME), b"not-json").unwrap();

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        assert_eq!(diagnosis.inventory.len(), 2, "{diagnosis:?}");
        assert!(codes(&diagnosis).contains(&FINDING_REALM_MANIFEST_UNREADABLE));
        let healthy = diagnosis
            .inventory
            .iter()
            .find(|e| e.realm == "healthy")
            .expect("healthy entry");
        assert_eq!(healthy.backend.as_deref(), Some("sqlite"));
        let corrupt = diagnosis
            .inventory
            .iter()
            .find(|e| e.realm == "corrupt")
            .expect("corrupt entry keyed by dir name");
        assert!(corrupt.backend.is_none());
        assert!(!diagnosis.has_errors() || diagnosis.count(FindingSeverity::Error) == 1);
    }

    #[tokio::test]
    async fn split_brain_twin_detected_across_roots() {
        let temp = tempfile::tempdir().unwrap();
        let root_a = temp.path().join("a");
        let root_b = temp.path().join("b");
        write_manifest(&root_a, "team", "sqlite");
        write_manifest(&root_b, "team", "sqlite");

        let diagnosis = diagnose_disk_roots(&scope(&[&root_a, &root_b])).await;
        let finding = diagnosis
            .findings
            .iter()
            .find(|f| f.code == FINDING_SPLIT_BRAIN_REALM)
            .expect("split-brain finding");
        assert_eq!(finding.severity, FindingSeverity::Error);
        assert!(finding.message.contains("team"));
        assert!(
            finding
                .message
                .contains(&root_a.join("team").display().to_string()),
            "{}",
            finding.message
        );
        assert!(
            finding
                .message
                .contains(&root_b.join("team").display().to_string()),
            "{}",
            finding.message
        );
        // Passing the same root twice must not fabricate a twin.
        let same = diagnose_disk_roots(&scope(&[&root_a, &root_a])).await;
        assert!(!codes(&same).contains(&FINDING_SPLIT_BRAIN_REALM));
        assert_eq!(same.inventory.len(), 1);
    }

    #[tokio::test]
    async fn realm_filter_restricts_the_sweep() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        write_manifest(&root, "alpha", "sqlite");
        write_manifest(&root, "beta", "sqlite");

        let diagnosis = diagnose_disk_roots(&scope(&[&root]).with_realm("alpha")).await;
        assert_eq!(diagnosis.inventory.len(), 1);
        assert_eq!(diagnosis.inventory[0].realm, "alpha");
    }

    #[tokio::test]
    async fn no_ledger_and_future_version_are_reported() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "aged", "sqlite");
        // Pre-ledger database: tables, no meerkat_schema.
        {
            let conn = Connection::open(realm_dir.join("sessions.sqlite3")).unwrap();
            conn.execute_batch(SESSIONS_DDL).unwrap();
        }
        // A future ledger version in a judgeable domain (jsonl-index is
        // visible feature-independently).
        {
            let index_dir = realm_dir.join("sessions_jsonl");
            std::fs::create_dir_all(&index_dir).unwrap();
            let conn = Connection::open(index_dir.join("session_index.sqlite3")).unwrap();
            conn.execute_batch(
                "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL)",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO meerkat_schema (domain, version) VALUES ('jsonl-index', 9999)",
                [],
            )
            .unwrap();
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        assert!(
            codes(&diagnosis).contains(&FINDING_NO_SCHEMA_LEDGER),
            "{diagnosis:?}"
        );
        let future = diagnosis
            .findings
            .iter()
            .find(|f| f.code == FINDING_SCHEMA_FROM_THE_FUTURE)
            .expect("future-version finding");
        assert_eq!(future.severity, FindingSeverity::Error);
        assert!(future.message.contains("jsonl-index"));
        assert!(future.message.contains("9999"));
        // Inventory carries the ledger rows and the row-less expected domains.
        let entry = &diagnosis.inventory[0];
        let sessions_db = entry
            .databases
            .iter()
            .find(|d| d.path.ends_with("sessions.sqlite3"))
            .expect("sessions db inventory");
        assert!(
            sessions_db
                .domains
                .iter()
                .all(|(_, version)| version.is_none())
        );
        let index_db = entry
            .databases
            .iter()
            .find(|d| d.path.ends_with("session_index.sqlite3"))
            .expect("index db inventory");
        assert!(
            index_db
                .domains
                .contains(&("jsonl-index".to_string(), Some(9999)))
        );
    }

    #[tokio::test]
    async fn census_counts_legacy_rows_and_jsonl_census_is_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "census", "sqlite");
        {
            let conn = Connection::open(realm_dir.join("sessions.sqlite3")).unwrap();
            conn.execute_batch(SESSIONS_DDL).unwrap();
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text("hello")));
            insert_session(&conn, &session);
        }
        let jsonl_root = temp.path().join("jsonl-realms");
        write_manifest(&jsonl_root, "journal", "jsonl");

        let diagnosis = diagnose_disk_roots(&scope(&[&root, &jsonl_root])).await;
        let legacy = diagnosis
            .findings
            .iter()
            .find(|f| f.code == FINDING_LEGACY_UNVERIFIED_SESSIONS)
            .expect("legacy census finding");
        assert_eq!(legacy.severity, FindingSeverity::Warning);
        assert!(legacy.message.starts_with("1 legacy-unverified"));
        assert_eq!(legacy.realm.as_deref(), Some("census"));
        let skipped = diagnosis
            .findings
            .iter()
            .find(|f| f.code == FINDING_CENSUS_SKIPPED_JSONL)
            .expect("jsonl census skip");
        assert_eq!(skipped.realm.as_deref(), Some("journal"));
    }

    #[tokio::test]
    async fn dangling_blob_reference_detected_and_present_blob_is_not_flagged() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "blobs", "sqlite");
        let missing_blob = BlobId::new(format!("sha256:{}", "a".repeat(64)));
        let present_blob = BlobId::new(format!("sha256:{}", "b".repeat(64)));
        // Materialize the present blob object per the Fs naming.
        let present_path = blob_object_path(&realm_dir.join("blobs"), &present_blob).unwrap();
        std::fs::create_dir_all(present_path.parent().unwrap()).unwrap();
        std::fs::write(&present_path, b"{}").unwrap();
        {
            let conn = Connection::open(realm_dir.join("sessions.sqlite3")).unwrap();
            conn.execute_batch(SESSIONS_DDL).unwrap();
            let mut session = Session::new();
            session.push(Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Blob {
                        blob_id: missing_blob.clone(),
                    },
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Blob {
                        blob_id: present_blob.clone(),
                    },
                },
            ])));
            insert_session(&conn, &session);
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        let dangling: Vec<_> = diagnosis
            .findings
            .iter()
            .filter(|f| f.code == FINDING_DANGLING_BLOB_REFERENCE)
            .collect();
        assert_eq!(dangling.len(), 1, "{diagnosis:?}");
        assert!(dangling[0].message.contains(missing_blob.as_str()));
        assert!(!dangling[0].message.contains(present_blob.as_str()));
        assert!(diagnosis.has_errors());
    }

    #[tokio::test]
    async fn carried_text_and_blob_json_columns_are_readable() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "carried", "sqlite");
        {
            let conn = Connection::open(realm_dir.join("sessions.sqlite3")).unwrap();
            conn.execute_batch(SESSIONS_DDL).unwrap();
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text("hello")));
            // A carried store: metadata as BLOB, session document as TEXT —
            // the swapped encodings an external host may have written; both
            // are valid JsonColumnBytes payloads.
            conn.execute(
                "INSERT INTO sessions (session_id, created_at_ms, updated_at_ms, message_count, \
                 total_tokens, metadata_json, session_json) \
                 VALUES (?1, 0, 0, 1, 0, CAST(?2 AS BLOB), ?3)",
                rusqlite::params![
                    session.id().to_string(),
                    serde_json::to_string(session.metadata()).unwrap(),
                    serde_json::to_string(&session).unwrap(),
                ],
            )
            .unwrap();
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        assert!(
            !codes(&diagnosis).contains(&FINDING_DATABASE_UNREADABLE),
            "{diagnosis:?}"
        );
        assert!(
            !codes(&diagnosis).contains(&FINDING_SESSION_DOCUMENT_UNDECODABLE),
            "{diagnosis:?}"
        );
        // The row still participates in the checkpoint census.
        assert!(codes(&diagnosis).contains(&FINDING_LEGACY_UNVERIFIED_SESSIONS));
    }

    #[tokio::test]
    async fn future_manifest_and_external_provider_realms_are_not_disk_swept() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        // Future manifest format over a database this binary must not judge
        // (the format may have relocated the real storage).
        let future_dir = root.join("future");
        std::fs::create_dir_all(&future_dir).unwrap();
        std::fs::write(
            future_dir.join(REALM_MANIFEST_FILE_NAME),
            serde_json::to_vec_pretty(&serde_json::json!({
                "realm_id": "future",
                "backend": "sqlite",
                "manifest_format": SUPPORTED_MANIFEST_FORMAT + 1,
                "created_at": "0",
            }))
            .unwrap(),
        )
        .unwrap();
        {
            let conn = Connection::open(future_dir.join("sessions.sqlite3")).unwrap();
            conn.execute_batch(SESSIONS_DDL).unwrap();
        }
        // Realm pinned to an external storage provider.
        let external_dir = root.join("remote");
        std::fs::create_dir_all(&external_dir).unwrap();
        std::fs::write(
            external_dir.join(REALM_MANIFEST_FILE_NAME),
            serde_json::to_vec_pretty(&serde_json::json!({
                "realm_id": "remote",
                "backend": "external:bigquery",
                "provider": "bigquery",
                "manifest_format": 2,
                "created_at": "0",
            }))
            .unwrap(),
        )
        .unwrap();

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        let future = diagnosis
            .findings
            .iter()
            .find(|f| f.code == FINDING_MANIFEST_FROM_THE_FUTURE)
            .expect("future-manifest finding");
        assert_eq!(future.severity, FindingSeverity::Error);
        assert_eq!(future.realm.as_deref(), Some("future"));
        let external = diagnosis
            .findings
            .iter()
            .find(|f| f.code == FINDING_EXTERNAL_PROVIDER_REALM)
            .expect("external-provider finding");
        assert_eq!(external.severity, FindingSeverity::Info);
        assert_eq!(external.realm.as_deref(), Some("remote"));
        // Neither realm gets a disk-shaped sweep: no database inventory, and
        // the future realm's sessions db is never inspected.
        assert_eq!(diagnosis.inventory.len(), 2);
        for entry in &diagnosis.inventory {
            assert!(entry.databases.is_empty(), "{entry:?}");
        }
        assert!(!codes(&diagnosis).contains(&FINDING_NO_SCHEMA_LEDGER));
    }

    #[tokio::test]
    async fn wrong_typed_manifest_and_database_paths_are_findings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "shapes", "sqlite");
        // A directory squatting on a required database path must not census
        // as merely absent.
        std::fs::create_dir_all(realm_dir.join("sessions.sqlite3")).unwrap();
        // A directory squatting on another realm's manifest path.
        let squatter = root.join("squatter");
        std::fs::create_dir_all(squatter.join(REALM_MANIFEST_FILE_NAME)).unwrap();

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        let wrong: Vec<_> = diagnosis
            .findings
            .iter()
            .filter(|f| f.code == FINDING_STORAGE_PATH_WRONG_TYPE)
            .collect();
        assert_eq!(wrong.len(), 2, "{diagnosis:?}");
        assert!(wrong.iter().all(|f| f.severity == FindingSeverity::Error));
        assert!(diagnosis.has_errors());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn broken_symlink_database_path_is_a_finding() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "links", "sqlite");
        std::os::unix::fs::symlink(
            realm_dir.join("nowhere.sqlite3"),
            realm_dir.join("workgraph.sqlite3"),
        )
        .unwrap();

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        let finding = diagnosis
            .findings
            .iter()
            .find(|f| f.code == FINDING_STORAGE_PATH_WRONG_TYPE)
            .expect("broken-symlink finding");
        assert!(
            finding.message.contains("broken symlink"),
            "{}",
            finding.message
        );
        assert_eq!(finding.severity, FindingSeverity::Error);
    }

    #[tokio::test]
    async fn higher_crate_domains_are_inventoried_and_unknown_domains_flagged() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "domains", "sqlite");
        {
            let conn = Connection::open(realm_dir.join("workgraph.sqlite3")).unwrap();
            conn.execute_batch(
                "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL)",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO meerkat_schema (domain, version) \
                 VALUES ('workgraph', 9999), ('from-mars', 3)",
                [],
            )
            .unwrap();
        }
        {
            let conn = Connection::open(realm_dir.join("jobs.sqlite3")).unwrap();
            conn.execute_batch(
                "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL)",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO meerkat_schema (domain, version) VALUES ('jobs', 1)",
                [],
            )
            .unwrap();
        }
        let mobs_dir = realm_dir.join("mobs");
        std::fs::create_dir_all(&mobs_dir).unwrap();
        {
            let conn = Connection::open(mobs_dir.join("alpha.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL)",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO meerkat_schema (domain, version) VALUES ('mob', 2)",
                [],
            )
            .unwrap();
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        // `workgraph` is owned above this crate: its version is inventoried
        // without judgment, however large.
        assert!(
            !codes(&diagnosis).contains(&FINDING_SCHEMA_FROM_THE_FUTURE),
            "{diagnosis:?}"
        );
        let unknown = diagnosis
            .findings
            .iter()
            .find(|f| f.code == FINDING_UNKNOWN_LEDGER_DOMAIN)
            .expect("unknown-domain finding");
        assert_eq!(unknown.severity, FindingSeverity::Warning);
        assert!(unknown.message.contains("from-mars"), "{}", unknown.message);
        let entry = &diagnosis.inventory[0];
        let workgraph_db = entry
            .databases
            .iter()
            .find(|d| d.path.ends_with("workgraph.sqlite3"))
            .expect("workgraph db inventory");
        assert!(
            workgraph_db
                .domains
                .contains(&("workgraph".to_string(), Some(9999)))
        );
        let jobs_db = entry
            .databases
            .iter()
            .find(|d| d.path.ends_with("jobs.sqlite3"))
            .expect("jobs db inventory");
        assert!(jobs_db.domains.contains(&("jobs".to_string(), Some(1))));
        let mob_db = entry
            .databases
            .iter()
            .find(|d| d.path.ends_with("mobs/alpha.db"))
            .expect("mob db inventory");
        assert!(mob_db.domains.contains(&("mob".to_string(), Some(2))));
    }

    #[tokio::test]
    async fn dangling_report_cap_dedups_and_counts_the_remainder() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "flood", "sqlite");
        let over = DANGLING_BLOB_REPORT_CAP + 2;
        {
            let conn = Connection::open(realm_dir.join("sessions.sqlite3")).unwrap();
            conn.execute_batch(SESSIONS_DDL).unwrap();
            let mut blocks = Vec::new();
            for i in 0..over {
                let blob = BlobId::new(format!("sha256:{i:064x}"));
                // Each reference twice: the report must count distinct pairs.
                for _ in 0..2 {
                    blocks.push(ContentBlock::Image {
                        media_type: "image/png".to_string(),
                        data: ImageData::Blob {
                            blob_id: blob.clone(),
                        },
                    });
                }
            }
            let mut session = Session::new();
            session.push(Message::User(UserMessage::with_blocks(blocks)));
            insert_session(&conn, &session);
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        let dangling: Vec<_> = diagnosis
            .findings
            .iter()
            .filter(|f| f.code == FINDING_DANGLING_BLOB_REFERENCE)
            .collect();
        // CAP individual findings plus exactly one remainder summary.
        assert_eq!(
            dangling.len(),
            DANGLING_BLOB_REPORT_CAP + 1,
            "{diagnosis:?}"
        );
        let summary = dangling.last().unwrap();
        assert!(
            summary.message.contains("2 additional"),
            "{}",
            summary.message
        );
        assert!(
            summary.message.contains(&format!("{over} total")),
            "{}",
            summary.message
        );
    }

    #[tokio::test]
    async fn undecodable_canonical_session_document_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "broken", "sqlite");
        {
            let conn = Connection::open(realm_dir.join("sessions.sqlite3")).unwrap();
            conn.execute_batch(SESSIONS_DDL).unwrap();
            let session = Session::new();
            conn.execute(
                "INSERT INTO sessions (session_id, created_at_ms, updated_at_ms, message_count, \
                 total_tokens, metadata_json, session_json) VALUES (?1, 0, 0, 0, 0, '{}', ?2)",
                rusqlite::params![session.id().to_string(), b"not-json".to_vec()],
            )
            .unwrap();
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        let finding = diagnosis
            .findings
            .iter()
            .find(|f| f.code == FINDING_SESSION_DOCUMENT_UNDECODABLE)
            .expect("undecodable finding");
        assert_eq!(finding.severity, FindingSeverity::Error);
        assert!(diagnosis.has_errors());
    }

    #[tokio::test]
    async fn artifact_and_lease_findings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "artifacts", "sqlite");
        std::fs::write(realm_dir.join("sessions.sqlite3.mfence"), b"").unwrap();
        std::fs::write(
            realm_dir.join("sessions.sqlite3.pre-1-1700000000"),
            b"backup",
        )
        .unwrap();
        let jsonl_dir = realm_dir.join("sessions_jsonl");
        std::fs::create_dir_all(&jsonl_dir).unwrap();
        std::fs::write(jsonl_dir.join("session_index.sqlite3.corrupt-123"), b"x").unwrap();
        // Backdated manifest lock (stale).
        let lock_path = realm_dir.join(".realm_manifest.lock");
        std::fs::write(&lock_path, b"realm-manifest-lock").unwrap();
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .open(&lock_path)
            .unwrap();
        lock.set_times(
            std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(3600)),
        )
        .unwrap();
        drop(lock);
        // Leases: one active, one stale, one unparseable.
        let lease_dir = realm_dir.join("leases");
        std::fs::create_dir_all(&lease_dir).unwrap();
        let lease = |heartbeat: u64| {
            serde_json::json!({
                "realm_id": "artifacts",
                "instance_id": "i",
                "surface": "rkat-rest",
                "pid": 42,
                "started_at": heartbeat,
                "heartbeat_at": heartbeat,
            })
        };
        std::fs::write(
            lease_dir.join("live.json"),
            serde_json::to_vec(&lease(now_unix_secs())).unwrap(),
        )
        .unwrap();
        std::fs::write(
            lease_dir.join("dead.json"),
            serde_json::to_vec(&lease(1)).unwrap(),
        )
        .unwrap();
        std::fs::write(lease_dir.join("garbage.json"), b"not-json").unwrap();

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        let found = codes(&diagnosis);
        for expected in [
            FINDING_MAINTENANCE_FENCE_LOCK,
            FINDING_BACKUP_ARTIFACT,
            FINDING_QUARANTINED_INDEX,
            FINDING_STALE_MANIFEST_LOCK,
            FINDING_ACTIVE_LEASE,
            FINDING_ORPHANED_LEASE,
            FINDING_UNPARSEABLE_LEASE,
        ] {
            assert!(found.contains(&expected), "missing {expected}: {found:?}");
        }
        // Artifact findings are inventory-grade or warnings, never errors.
        assert!(!diagnosis.has_errors(), "{diagnosis:?}");
    }

    #[tokio::test]
    async fn wrong_typed_realms_root_is_an_error_finding() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        // A file squatting on the realms root must not yield a clean report.
        std::fs::write(&root, b"not a directory").unwrap();

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        let finding = diagnosis
            .findings
            .iter()
            .find(|f| f.code == FINDING_STORAGE_PATH_WRONG_TYPE)
            .expect("wrong-typed root finding");
        assert_eq!(finding.severity, FindingSeverity::Error);
        assert!(diagnosis.has_errors());
        // An absent root stays a normal state, not a finding.
        let absent = diagnose_disk_roots(&scope(&[&temp.path().join("missing")])).await;
        assert!(absent.findings.is_empty(), "{absent:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unreadable_realms_root_is_an_error_finding() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&root).is_ok() {
            // Permission bits are not enforced here (e.g. running as root);
            // nothing to assert.
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        // Restore before asserting so tempdir cleanup succeeds either way.
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let finding = diagnosis
            .findings
            .iter()
            .find(|f| f.code == FINDING_STATE_ROOT_UNREADABLE)
            .expect("unreadable root finding");
        assert_eq!(finding.severity, FindingSeverity::Error);
        assert!(diagnosis.has_errors());
    }

    #[tokio::test]
    async fn first_start_markers_are_info_when_recent_and_warning_when_stale() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        std::fs::create_dir_all(&root).unwrap();
        write_manifest(&root, "team", "sqlite");
        // Recent marker: the payload timestamp governs.
        std::fs::write(
            root.join(".realm-first-start.team.lock"),
            serde_json::to_vec(&serde_json::json!({
                "realm_id": "team",
                "pid": 42,
                "created_at_unix": now_unix_secs(),
            }))
            .unwrap(),
        )
        .unwrap();
        // Stale marker: payload timestamp far past the horizon.
        std::fs::write(
            root.join(".realm-first-start.old-team.lock"),
            serde_json::to_vec(&serde_json::json!({
                "realm_id": "old-team",
                "pid": 42,
                "created_at_unix": 1,
            }))
            .unwrap(),
        )
        .unwrap();

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;
        let markers: Vec<_> = diagnosis
            .findings
            .iter()
            .filter(|f| f.code == FINDING_FIRST_START_MARKER)
            .collect();
        assert_eq!(markers.len(), 2, "{diagnosis:?}");
        let recent = markers
            .iter()
            .find(|f| f.realm.as_deref() == Some("team"))
            .expect("recent marker finding");
        assert_eq!(recent.severity, FindingSeverity::Info);
        let stale = markers
            .iter()
            .find(|f| f.realm.as_deref() == Some("old-team"))
            .expect("stale marker finding");
        assert_eq!(stale.severity, FindingSeverity::Warning);
        assert!(
            stale.message.contains("age-based takeover"),
            "{}",
            stale.message
        );
        // Marker findings alone never make the report an error.
        assert!(!diagnosis.has_errors(), "{diagnosis:?}");

        // The realm filter applies to markers like it does to realm dirs.
        let filtered = diagnose_disk_roots(&scope(&[&root]).with_realm("old-team")).await;
        let filtered_markers: Vec<_> = filtered
            .findings
            .iter()
            .filter(|f| f.code == FINDING_FIRST_START_MARKER)
            .collect();
        assert_eq!(filtered_markers.len(), 1, "{filtered:?}");
        assert_eq!(filtered_markers[0].realm.as_deref(), Some("old-team"));
    }

    #[tokio::test]
    async fn explicit_roots_are_the_only_thing_read() {
        // Hermeticity: a realm outside the scoped roots is invisible.
        let temp = tempfile::tempdir().unwrap();
        let scoped = temp.path().join("scoped");
        let unscoped = temp.path().join("unscoped");
        write_manifest(&scoped, "inside", "sqlite");
        write_manifest(&unscoped, "outside", "sqlite");

        let diagnosis = diagnose_disk_roots(&scope(&[&scoped])).await;
        assert_eq!(diagnosis.inventory.len(), 1);
        assert_eq!(diagnosis.inventory[0].realm, "inside");
    }

    #[tokio::test]
    async fn disk_storage_migrator_delegates() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        write_manifest(&root, "seam", "sqlite");
        let migrator = DiskStorageMigrator;
        let diagnosis = migrator
            .diagnose(&scope(&[&root]))
            .await
            .expect("diagnose never fails on disk");
        assert_eq!(diagnosis.inventory.len(), 1);
        assert_eq!(diagnosis.inventory[0].realm, "seam");
    }
}

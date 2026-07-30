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
//!
//! # Storage census
//!
//! [`census_storage_footprint`] measures what each session actually costs on
//! disk — durable bytes against live-transcript bytes, the
//! `session_strand_messages` pool split into live prefixes and retained
//! copies (with `session_strand_links` supersessions counted so a
//! spliced-away strand still shows up as a retained revision), and the
//! frozen `sessions.session_json` archives behind head rows. Inline
//! WholeBlob graphs are measured through borrowed raw JSON: current compact
//! anchors and occurrence edges are reported separately from exact
//! released-0.8.10 full-body graphs, without typed body materialization.
//! Byte counts come from SQL `LENGTH(CAST(<column> AS BLOB))` over the
//! durable columns (byte-exact for TEXT-or-BLOB storage) or, for an inline
//! WholeBlob document, from the raw serialized JSON — never from a typed
//! re-serialization, which would report bytes the disk does not hold.
//!
//! The census is fail-closed: a row it cannot measure is counted, excluded
//! from every aggregate, and reported as
//! [`FINDING_STORAGE_CENSUS_UNMEASURED`]. It never reports an unmeasured
//! pool as healthy, and every message names the tables it measured so the
//! report cannot be read as covering a pool it never touched.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use meerkat_core::session::TRANSCRIPT_HISTORY_FORMAT_CURRENT;
use meerkat_core::storage_diagnostics::{
    DatabaseInventory, DiagnoseScope, FindingSeverity, StorageDiagnosis, StorageDiagnosticsError,
    StorageFinding, StorageInventoryEntry, StorageMigrator,
};
use meerkat_core::{
    BlobId, ContentBlock, Message, REALM_MANIFEST_FILE_NAME, SessionId, SystemNoticeBlock,
    sanitize_realm_id, validate_current_persisted_transcript_history_slice,
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
/// Persisted session/message projections that do not decode during the blob
/// sweep (error severity).
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
/// A session whose durable storage is disproportionate to the live
/// transcript it serves (retained transcript history dominates the
/// document).
pub const FINDING_TRANSCRIPT_HISTORY_OVERSIZED: &str = "transcript-history-oversized";
/// The `session_strand_messages` pool, split into the live head-strand
/// prefixes and the retained non-live transcript copies. Reported even when
/// healthy: an unmeasured pool is how 74.9x inflation went unnoticed.
pub const FINDING_STRAND_DUPLICATION_RECLAIMABLE: &str = "strand-duplication-reclaimable";
/// `sessions.session_json` bytes retained for sessions that already have a
/// `session_heads` row (frozen archives the session store never reads).
pub const FINDING_FROZEN_BLOB_ARCHIVE_RECLAIMABLE: &str = "frozen-blob-archive-reclaimable";
/// Raw inline transcript-graph footprint: compact current anchor/occurrence
/// edges and exact released-0.8.10 full-body documents are reported as
/// physically distinct representations.
pub const FINDING_INLINE_TRANSCRIPT_GRAPH_FOOTPRINT: &str = "inline-transcript-graph-footprint";
/// Exact released-0.8.10 graphs were structurally measured but their
/// arbitrary-base full-body semantics are intentionally proved only by the
/// one-time persisted-session ingress/migration authority.
pub const FINDING_RELEASED_TRANSCRIPT_GRAPH_SEMANTIC_VERIFICATION_DEFERRED: &str =
    "released-transcript-graph-semantic-verification-deferred";
/// Authenticated per-key HeadCanonical metadata cells, immutable state
/// transitions, and the physical/runtime owner references that keep exact
/// states reachable.
pub const FINDING_HEAD_METADATA_SIDECAR: &str = "head-metadata-sidecar";
/// Durable rows or documents the storage census could not measure. The
/// footprint findings exclude them, so the report says *unknown* instead of
/// implying the unmeasured bytes are healthy.
pub const FINDING_STORAGE_CENSUS_UNMEASURED: &str = "storage-census-unmeasured";

/// Cap on individually reported dangling blob references per database; the
/// remainder is summarized in one finding so doctor stays usable on huge
/// realms.
const DANGLING_BLOB_REPORT_CAP: usize = 50;

/// Warn threshold for one session's durable bytes ÷ live transcript bytes.
///
/// A session that stores each live message once sits at ~1.0 (a small head
/// row plus one strand copy). Legitimate retention is bounded: one adopted
/// rewrite retains at most one extra full transcript (~2x), and a handful
/// of compactions add a few x more. The production regression this census
/// exists to catch ran at 63.8x for one identity and 74.9x fleet-wide
/// (worst session 118x), so 4.0 sits well above ordinary retention and far
/// below every observed defect value.
const TRANSCRIPT_HISTORY_RATIO_WARN: f64 = 4.0;

/// Floor under which no footprint finding is raised, whatever the ratio:
/// a small session's fixed head/metadata overhead can dwarf a two-message
/// transcript with nothing wrong, and there is no operator action worth
/// taking below a mebibyte of reclaimable bytes.
const STORAGE_CENSUS_RECLAIMABLE_FLOOR_BYTES: u64 = 1 << 20;

/// Warn threshold for the strand pool as a whole: at 2.0 the pool holds as
/// many retained bytes as live ones, i.e. duplication rather than
/// conversation has become the majority of the durable transcript store.
const STRAND_DUPLICATION_RATIO_WARN: f64 = 2.0;

/// Cap on individually reported oversized sessions per database; the
/// remainder is summarized (same discipline as
/// [`DANGLING_BLOB_REPORT_CAP`], lower because each line is verbose).
const TRANSCRIPT_HISTORY_REPORT_CAP: usize = 20;

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
    // Lazily provisioned by the first durable-delivery write; absent on
    // realms that never used durable delivery, so it is deliberately NOT in
    // any `REALM_DATABASE_FILES` expected-domain list.
    "runtime-delivery",
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
                        // One deferred read transaction so the blob sweep and
                        // storage-footprint census observe a single SQLite
                        // snapshot: a live
                        // legacy-to-strand migration landing between separate
                        // autocommit queries could otherwise move a session
                        // out of both views (or let the footprint census
                        // charge one session both its strand rows and the
                        // blob row those rows were just built from). The
                        // first SELECT inside the transaction establishes the
                        // snapshot.
                        match conn.unchecked_transaction() {
                            Ok(tx) => {
                                sweep_dangling_blobs(
                                    &tx,
                                    realm_dir,
                                    &sessions_db,
                                    realm,
                                    diagnosis,
                                );
                                census_storage_footprint(&tx, &sessions_db, realm, diagnosis);
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
/// live messages and strand messages, walk them for
/// `ImageData::Blob { blob_id }`, and probe the realm's `blobs/` directory
/// for each referenced object. Callers pass a connection holding the
/// per-database read snapshot (see `diagnose_realm_dir`).
///
/// Inline documents are read through a raw borrowed `messages`/metadata lens.
/// Current compact graphs cross core's raw persisted-graph verifier before
/// live messages are inspected, so dropping typed [`meerkat_core::Session`]
/// hydration does not also drop current-graph corruption detection. Released
/// 0.8.10 graphs get an exact raw physical-shape/footprint check here; their
/// arbitrary-base rebase semantics remain with the one-time full-ingress
/// authority instead of being normalized by a read-only diagnostic.
#[derive(Deserialize)]
struct SessionLiveMessagesLens<'a> {
    #[serde(borrow)]
    messages: Vec<&'a serde_json::value::RawValue>,
    #[serde(borrow, default)]
    metadata: BTreeMap<String, &'a serde_json::value::RawValue>,
}

fn collect_inline_session_blob_refs(document: &[u8]) -> Result<Vec<BlobId>, ()> {
    let lens: SessionLiveMessagesLens<'_> = serde_json::from_slice(document).map_err(|_| ())?;
    if let Some(graph) = lens.metadata.get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY) {
        let format: InlineHistoryFormatLens<'_> =
            serde_json::from_str(graph.get()).map_err(|_| ())?;
        match format.format {
            Some(TRANSCRIPT_HISTORY_FORMAT_CURRENT) => {
                validate_current_persisted_transcript_history_slice(graph.get().as_bytes())
                    .map_err(|_| ())?;
            }
            Some(_) => return Err(()),
            // Released 0.8.10 rebase entries may reference arbitrary earlier
            // bodies, so exact semantic verification is intentionally the
            // one-time full-ingress/migration authority. Doctor checks the
            // frozen physical wire shape and reports its full-body footprint
            // without normalizing or retaining every historical body.
            None if measure_inline_history(graph).is_some() => {}
            None => return Err(()),
        }
    }
    let mut refs = Vec::new();
    for raw in lens.messages {
        let message: Message = serde_json::from_str(raw.get()).map_err(|_| ())?;
        collect_message_blob_refs(&message, &mut refs);
    }
    Ok(refs)
}

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
                match collect_inline_session_blob_refs(&session_json.into_bytes()) {
                    Ok(refs) => collector.record(&blobs_root, &session_id, refs),
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
        // Error severity: these are canonical live-message representations
        // (strand rows, or the messages/graph projection of a blob row with no
        // head). Current inline graphs cross core's raw verifier; released
        // graphs cross the frozen physical-shape lens and retain their exact
        // semantic authority at full ingress. Doctor itself does not normalize
        // them or own a parallel graph-validation algorithm.
        diagnosis.findings.push(
            StorageFinding::new(
                FindingSeverity::Error,
                FINDING_SESSION_DOCUMENT_UNDECODABLE,
                format!(
                    "{undecodable} persisted live-message or transcript-graph projection(s) did \
                     not validate during the blob-reference sweep"
                ),
            )
            .with_path(db_path.to_path_buf())
            .with_realm(realm),
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Storage footprint census: durable bytes vs live conversation.
// ─────────────────────────────────────────────────────────────────────────

/// The head row's classification key for the strand pass.
#[derive(Debug)]
struct HeadKey {
    /// `session_heads.strand`: the strand carrying the live transcript.
    strand: String,
    /// `session_heads.message_count`: rows `0..message_count` of that strand
    /// are the live transcript; everything else is retained history.
    message_count: u64,
}

/// One session's measured durable footprint, in bytes actually stored.
#[derive(Debug, Default)]
struct SessionFootprint {
    /// Head-row identity. `None` means no usable `session_heads` row, so the
    /// session is an inline WholeBlob document (or its head row is
    /// unmeasurable).
    head: Option<HeadKey>,
    /// `session_heads.head_json` + `session_heads.metadata_json`.
    head_bytes: u64,
    /// Immutable JSON payload bytes in `session_head_metadata_cells` and
    /// `session_head_metadata_states`. Reference, delta, and lineage rows are
    /// counted separately but have no JSON payload to byte-measure.
    head_metadata_bytes: u64,
    /// Every `session_strand_messages` row for this session.
    strand_bytes: u64,
    /// The live head-strand prefix — the transcript a resume actually loads.
    live_strand_bytes: u64,
    /// Distinct strands carrying at least one measured row.
    strands: u64,
    /// `session_rewrites.commit_json`.
    rewrite_bytes: u64,
    /// `session_rewrites` rows.
    rewrite_rows: u64,
    /// `sessions.session_json`: the live document for a legacy session, a
    /// frozen archive for a head-canonical one. Durable either way.
    blob_bytes: u64,
    /// Legacy inline document only: the serialized live `messages` array.
    inline_live_bytes: u64,
    /// Inline document only: exact serialized compact anchor payload.
    inline_graph_anchor_bytes: u64,
    /// Inline document only: exact serialized compact occurrence-edge
    /// payloads.
    inline_graph_edge_bytes: u64,
    /// Inline document only: compact occurrence edges (one per rewrite).
    inline_graph_edges: u64,
    /// Inline document only: exact serialized transcript-history graph,
    /// including its rolling-prefix witnesses.
    inline_graph_bytes: u64,
    /// Exact released-0.8.10 inline document only: full revision bodies.
    inline_released_revision_bodies: u64,
    /// Exact released-0.8.10 inline document only: rewrite commits.
    inline_released_commits: u64,
    /// The inline graph's physical representation. `None` means there is no
    /// transcript-history graph.
    inline_graph_kind: Option<InlineGraphKind>,
    /// `session_strand_messages` rows whose session has no usable
    /// `session_heads` row. `append_messages` lands strand rows and
    /// `save_head` mints the head row in a separate transaction
    /// (`meerkat-store/src/sqlite_store.rs`), so this is a legitimate
    /// durable state, not corruption — but without the head key the rows'
    /// live-or-retained split is unknowable. The strand pool counts the
    /// same rows in its unclassified bucket.
    headless_strand_rows: u64,
    /// Some durable part of this session could not be measured, so it is
    /// excluded from every ratio finding (reported unknown, never healthy).
    unmeasured: bool,
}

impl SessionFootprint {
    /// Head-canonical sessions are measured across the head/strand/rewrite
    /// tables; the rest are inline WholeBlob documents.
    fn head_canonical(&self) -> bool {
        self.head.is_some()
    }

    /// Every durable byte the session owns, across every measured table.
    fn document_bytes(&self) -> u64 {
        self.head_bytes
            .saturating_add(self.head_metadata_bytes)
            .saturating_add(self.strand_bytes)
            .saturating_add(self.rewrite_bytes)
            .saturating_add(self.blob_bytes)
    }

    /// The transcript a resume loads — the only bytes the conversation needs.
    fn live_bytes(&self) -> u64 {
        if self.head_canonical() {
            self.live_strand_bytes
        } else {
            self.inline_live_bytes
        }
    }

    fn reclaimable_bytes(&self) -> u64 {
        self.document_bytes().saturating_sub(self.live_bytes())
    }

    /// Retained revisions besides the live one: one per non-head strand for
    /// a head-canonical session.
    fn retained_revisions(&self) -> u64 {
        self.strands.saturating_sub(1)
    }

    fn rewrite_commits(&self) -> u64 {
        self.rewrite_rows
    }

    /// Describe exactly what the inline document stores. Current compact
    /// graphs retain one anchor and occurrence deltas; they do not retain one
    /// full transcript body per revision.
    fn inline_graph_description(&self) -> String {
        match self.inline_graph_kind {
            Some(InlineGraphKind::Compact) => format!(
                "compact transcript graph inline in sessions.session_json: 1 anchor / {}, {} \
                 occurrence edge(s) / {}, {} total graph bytes including rolling witnesses",
                format_bytes(self.inline_graph_anchor_bytes),
                self.inline_graph_edges,
                format_bytes(self.inline_graph_edge_bytes),
                format_bytes(self.inline_graph_bytes),
            ),
            Some(InlineGraphKind::Released0810) => format!(
                "released-0.8.10 full-body transcript graph inline in sessions.session_json: {} \
                 revision bod(ies), {} rewrite commit(s), {} total graph bytes",
                self.inline_released_revision_bodies,
                self.inline_released_commits,
                format_bytes(self.inline_graph_bytes),
            ),
            None => "no inline transcript-history graph".to_string(),
        }
    }

    /// A session holding strand rows with no usable head row. Its strand
    /// bytes have an unknowable live-or-retained split — the head save that
    /// would classify them has not landed (or did not read back) — so every
    /// ratio finding must fail closed and exclude it: reading it as a
    /// inline WholeBlob document would claim bytes live in
    /// `sessions.session_json` for a session that may have no row there at
    /// all, and firing the mass-alone arm would treat "live is unknown" as
    /// "live is zero".
    fn split_unclassifiable(&self) -> bool {
        !self.head_canonical() && self.headless_strand_rows > 0
    }

    /// Warn-worthy: fully measured, at least [the floor] of reclaimable
    /// bytes, and past the ratio. A session with a KNOWN-empty live
    /// transcript cannot have a ratio, so mass alone decides there; a
    /// session whose live bytes are unknowable ([`split_unclassifiable`])
    /// is excluded outright rather than guessed at.
    ///
    /// [the floor]: STORAGE_CENSUS_RECLAIMABLE_FLOOR_BYTES
    /// [`split_unclassifiable`]: SessionFootprint::split_unclassifiable
    fn is_oversized(&self) -> bool {
        if self.unmeasured
            || self.split_unclassifiable()
            || self.reclaimable_bytes() < STORAGE_CENSUS_RECLAIMABLE_FLOOR_BYTES
        {
            return false;
        }
        let live = self.live_bytes();
        live == 0 || self.document_bytes() as f64 / live as f64 >= TRANSCRIPT_HISTORY_RATIO_WARN
    }
}

/// The `session_strand_messages` pool, classified against the head rows.
#[derive(Debug, Default)]
struct StrandPoolCensus {
    sessions: u64,
    rows: u64,
    bytes: u64,
    live_rows: u64,
    live_bytes: u64,
    retained_rows: u64,
    retained_bytes: u64,
    /// Rows whose session has no usable head row: real durable bytes that
    /// cannot be classified live-or-retained, so they are never counted as
    /// either.
    unclassified_rows: u64,
    unclassified_bytes: u64,
    /// `session_strand_links` supersession rows. Counted, not byte-measured:
    /// the row carries no JSON payload, only the splice span that lets a
    /// superseded strand stop storing a whole transcript copy.
    links: u64,
}

/// Frozen `sessions.session_json` rows behind a `session_heads` row.
#[derive(Debug, Default)]
struct FrozenArchiveCensus {
    sessions: u64,
    bytes: u64,
    /// Rows in a legacy `runtime_session_snapshots` table sharing this
    /// database file (see [`legacy_runtime_snapshot_rows`]).
    legacy_runtime_snapshots: u64,
}

/// Authenticated HeadCanonical metadata cells, state transitions, and exact
/// owner references. JSON payload bytes are measured with SQLite `LENGTH`;
/// doctor never parses or allocates the accumulated metadata map.
#[derive(Debug, Default)]
struct HeadMetadataPoolCensus {
    cell_rows: u64,
    cell_bytes: u64,
    current_rows: u64,
    state_rows: u64,
    state_identity_bytes: u64,
    delta_rows: u64,
    physical_refs: u64,
    runtime_refs: u64,
    unknown_refs: u64,
    lineage_rows: u64,
}

/// What the census could not measure (fail-closed accounting).
#[derive(Debug, Default)]
struct CensusGaps {
    /// Durable rows whose length/count columns did not read back as a
    /// non-negative integer.
    rows: u64,
    /// Legacy inline documents whose JSON did not parse far enough to split
    /// live transcript from retained history.
    documents: u64,
    /// Sessions holding `session_strand_messages` rows but no usable
    /// `session_heads` row ([`SessionFootprint::split_unclassifiable`]):
    /// their live-or-retained split is unknowable, so the ratio findings
    /// exclude them and this census says so instead of certifying them
    /// healthy. The strand pool's unclassified bucket carries the same
    /// rows' bytes.
    headless_strand_sessions: u64,
    /// Pools whose census query failed outright (each also reported as
    /// [`FINDING_DATABASE_UNREADABLE`]).
    pools: Vec<&'static str>,
}

impl CensusGaps {
    fn is_empty(&self) -> bool {
        self.rows == 0
            && self.documents == 0
            && self.headless_strand_sessions == 0
            && self.pools.is_empty()
    }

    /// Record a pool the census could not query at all, and report it with
    /// the same code the rest of doctor uses for an unreadable database.
    fn pool_unreadable(
        &mut self,
        pool: &'static str,
        error: &rusqlite::Error,
        db_path: &Path,
        realm: &str,
        diagnosis: &mut StorageDiagnosis,
    ) {
        self.pools.push(pool);
        diagnosis.findings.push(
            StorageFinding::new(
                FindingSeverity::Error,
                FINDING_DATABASE_UNREADABLE,
                format!("storage census query over `{pool}` failed: {error}"),
            )
            .with_path(db_path.to_path_buf())
            .with_realm(realm),
        );
    }

    /// Suffix every aggregate message carries when the census is partial, so
    /// no number in this report can be read as complete when it is not.
    fn exclusion_note(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        format!(
            " — partial census: {} unmeasurable row(s), {} unmeasurable document(s), {} \
             head-less strand session(s) and {} unreadable pool(s) are excluded from these \
             numbers (see `{FINDING_STORAGE_CENSUS_UNMEASURED}`)",
            self.rows,
            self.documents,
            self.headless_strand_sessions,
            self.pools.len()
        )
    }
}

/// A durable count/length column read fail-closed: NULL or negative is not a
/// measurement.
fn measured_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

/// Byte counts as an operator reads them.
fn format_bytes(bytes: u64) -> String {
    const GIB: u64 = 1 << 30;
    const MIB: u64 = 1 << 20;
    const KIB: u64 = 1 << 10;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// `durable ÷ live`, or an explicit "no live bytes" — never a fabricated
/// ratio over a zero denominator.
fn format_ratio(numerator: u64, denominator: u64) -> String {
    if denominator == 0 {
        return "n/a, no live bytes measured".to_string();
    }
    format!("{:.1}x", numerator as f64 / denominator as f64)
}

/// Read-only storage-footprint census over one sqlite session database (see
/// the module docs). Callers pass a connection holding the per-database read
/// snapshot, so the passes below agree with each other and with the blob
/// sweep that shares it.
fn census_storage_footprint(
    conn: &Connection,
    db_path: &Path,
    realm: &str,
    diagnosis: &mut StorageDiagnosis,
) {
    let mut sessions: BTreeMap<String, SessionFootprint> = BTreeMap::new();
    let mut strand_pool = StrandPoolCensus::default();
    let mut archives = FrozenArchiveCensus::default();
    let mut head_metadata = HeadMetadataPoolCensus::default();
    let mut gaps = CensusGaps::default();

    // Order matters: the head pass establishes the classification key the
    // strand and blob passes need. A pool that fails is reported and skipped;
    // the rest of the census still runs and says so.
    if let Err(error) = census_head_rows(conn, &mut sessions, &mut gaps) {
        gaps.pool_unreadable("session_heads", &error, db_path, realm, diagnosis);
    }
    if let Err(error) = census_strand_rows(conn, &mut sessions, &mut strand_pool, &mut gaps) {
        gaps.pool_unreadable("session_strand_messages", &error, db_path, realm, diagnosis);
    }
    if let Err(error) = census_strand_links(conn, &mut sessions, &mut strand_pool) {
        gaps.pool_unreadable("session_strand_links", &error, db_path, realm, diagnosis);
    }
    if let Err(error) = census_rewrite_rows(conn, &mut sessions, &mut gaps) {
        gaps.pool_unreadable("session_rewrites", &error, db_path, realm, diagnosis);
    }
    if let Err(error) = census_blob_rows(conn, &mut sessions, &mut archives, &mut gaps) {
        gaps.pool_unreadable("sessions", &error, db_path, realm, diagnosis);
    }
    if let Err(error) =
        census_head_metadata_rows(conn, &mut sessions, &mut head_metadata, &mut gaps)
    {
        gaps.pool_unreadable(
            "session_head_metadata_cells + session_head_metadata_current + \
             session_head_metadata_states + session_head_metadata_state_deltas + \
             session_head_metadata_refs + session_head_metadata_head_lineage",
            &error,
            db_path,
            realm,
            diagnosis,
        );
    }
    // Classification, not measurement, is what failed for these sessions —
    // but the consequence is the same: their footprint is unknown, and every
    // finding below must carry that instead of certifying them healthy.
    gaps.headless_strand_sessions = sessions
        .values()
        .filter(|footprint| footprint.split_unclassifiable())
        .count() as u64;

    report_oversized_sessions(&sessions, &gaps, db_path, realm, diagnosis);
    report_inline_graph_pool(&sessions, &gaps, db_path, realm, diagnosis);
    report_strand_pool(&strand_pool, &gaps, db_path, realm, diagnosis);
    report_frozen_archives(&archives, &gaps, db_path, realm, diagnosis);
    report_head_metadata_pool(&head_metadata, &gaps, db_path, realm, diagnosis);
    report_census_gaps(&gaps, db_path, realm, diagnosis);
}

/// Head rows: the classification key (live strand + prefix length) and the
/// head row's own durable bytes.
fn census_head_rows(
    conn: &Connection,
    sessions: &mut BTreeMap<String, SessionFootprint>,
    gaps: &mut CensusGaps,
) -> Result<(), rusqlite::Error> {
    if !table_exists(conn, "session_heads")? {
        return Ok(());
    }
    let mut statement = conn.prepare(
        "SELECT session_id, strand, message_count, \
         LENGTH(CAST(head_json AS BLOB)), LENGTH(CAST(metadata_json AS BLOB)) \
         FROM session_heads ORDER BY session_id",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let session_id: String = row.get(0)?;
        let strand: Option<String> = row.get(1)?;
        let message_count = measured_u64(row.get(2)?);
        let head_json = measured_u64(row.get(3)?);
        let metadata_json = measured_u64(row.get(4)?);
        let footprint = sessions.entry(session_id).or_default();
        match (strand, message_count) {
            (Some(strand), Some(message_count)) => {
                footprint.head = Some(HeadKey {
                    strand,
                    message_count,
                });
            }
            _ => {
                footprint.unmeasured = true;
                gaps.rows += 1;
            }
        }
        match (head_json, metadata_json) {
            (Some(head_json), Some(metadata_json)) => {
                footprint.head_bytes = head_json.saturating_add(metadata_json);
            }
            _ => {
                footprint.unmeasured = true;
                gaps.rows += 1;
            }
        }
    }
    Ok(())
}

/// Strand rows: the durable transcript pool, split per session into the live
/// head-strand prefix and the retained copies every rewrite adds.
fn census_strand_rows(
    conn: &Connection,
    sessions: &mut BTreeMap<String, SessionFootprint>,
    pool: &mut StrandPoolCensus,
    gaps: &mut CensusGaps,
) -> Result<(), rusqlite::Error> {
    if !table_exists(conn, "session_strand_messages")? {
        return Ok(());
    }
    // Grouped by (session, strand) so distinct strands are counted by
    // transition instead of a per-session set.
    let mut statement = conn.prepare(
        "SELECT session_id, strand, seq, LENGTH(CAST(message_json AS BLOB)) \
         FROM session_strand_messages ORDER BY session_id, strand, seq",
    )?;
    let mut rows = statement.query([])?;
    let mut current_session: Option<String> = None;
    let mut current_strand: Option<String> = None;
    while let Some(row) = rows.next()? {
        let session_id: String = row.get(0)?;
        let strand: String = row.get(1)?;
        let seq = measured_u64(row.get(2)?);
        let bytes = measured_u64(row.get(3)?);
        let (Some(seq), Some(bytes)) = (seq, bytes) else {
            sessions.entry(session_id).or_default().unmeasured = true;
            gaps.rows += 1;
            continue;
        };
        if current_session.as_ref() != Some(&session_id) {
            pool.sessions += 1;
            current_session = Some(session_id.clone());
            current_strand = None;
        }
        if current_strand.as_ref() != Some(&strand) {
            sessions.entry(session_id.clone()).or_default().strands += 1;
            current_strand = Some(strand.clone());
        }
        let footprint = sessions.entry(session_id).or_default();
        footprint.strand_bytes = footprint.strand_bytes.saturating_add(bytes);
        pool.rows += 1;
        pool.bytes = pool.bytes.saturating_add(bytes);
        match &footprint.head {
            Some(head) if head.strand == strand && seq < head.message_count => {
                footprint.live_strand_bytes = footprint.live_strand_bytes.saturating_add(bytes);
                pool.live_rows += 1;
                pool.live_bytes = pool.live_bytes.saturating_add(bytes);
            }
            Some(_) => {
                pool.retained_rows += 1;
                pool.retained_bytes = pool.retained_bytes.saturating_add(bytes);
            }
            None => {
                footprint.headless_strand_rows += 1;
                pool.unclassified_rows += 1;
                pool.unclassified_bytes = pool.unclassified_bytes.saturating_add(bytes);
            }
        }
    }
    Ok(())
}

/// Supersession links: strands that keep only their divergent span and
/// resolve the rest through a successor.
///
/// Censused for two reasons. A strand whose span was spliced away entirely
/// owns no `session_strand_messages` row, so without this pass it would
/// vanish from the retained-revision count; and a pool nobody counts is a
/// pool the report would implicitly claim does not exist.
fn census_strand_links(
    conn: &Connection,
    sessions: &mut BTreeMap<String, SessionFootprint>,
    pool: &mut StrandPoolCensus,
) -> Result<(), rusqlite::Error> {
    if !table_exists(conn, "session_strand_links")? {
        return Ok(());
    }
    // A link whose strand still owns physical rows was already counted by the
    // strand pass; only the fully spliced-away strands are added here.
    let probe_sql = "SELECT 1 FROM session_strand_messages \
                     WHERE session_id = ?1 AND strand = ?2 LIMIT 1";
    let mut materialized = if table_exists(conn, "session_strand_messages")? {
        Some(conn.prepare(probe_sql)?)
    } else {
        None
    };
    let mut statement = conn.prepare(
        "SELECT session_id, strand FROM session_strand_links ORDER BY session_id, strand",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let session_id: String = row.get(0)?;
        let strand: String = row.get(1)?;
        let has_rows = match materialized.as_mut() {
            Some(probe) => probe.exists(rusqlite::params![session_id, strand])?,
            None => false,
        };
        pool.links += 1;
        if !has_rows {
            sessions.entry(session_id).or_default().strands += 1;
        }
    }
    Ok(())
}

/// Rewrite commit rows (the adopted-commit ledger, not the bodies).
fn census_rewrite_rows(
    conn: &Connection,
    sessions: &mut BTreeMap<String, SessionFootprint>,
    gaps: &mut CensusGaps,
) -> Result<(), rusqlite::Error> {
    if !table_exists(conn, "session_rewrites")? {
        return Ok(());
    }
    let mut statement = conn.prepare(
        "SELECT session_id, LENGTH(CAST(commit_json AS BLOB)) FROM session_rewrites \
         ORDER BY session_id, rewrite_idx",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let session_id: String = row.get(0)?;
        let bytes = measured_u64(row.get(1)?);
        let footprint = sessions.entry(session_id).or_default();
        footprint.rewrite_rows += 1;
        match bytes {
            Some(bytes) => footprint.rewrite_bytes = footprint.rewrite_bytes.saturating_add(bytes),
            None => {
                footprint.unmeasured = true;
                gaps.rows += 1;
            }
        }
    }
    Ok(())
}

/// Head metadata sidecar rows. Only row counts and SQLite-reported JSON byte
/// lengths are read; cells and state identities are never selected or parsed.
fn census_head_metadata_rows(
    conn: &Connection,
    sessions: &mut BTreeMap<String, SessionFootprint>,
    pool: &mut HeadMetadataPoolCensus,
    gaps: &mut CensusGaps,
) -> Result<(), rusqlite::Error> {
    if table_exists(conn, "session_head_metadata_cells")? {
        let mut statement = conn.prepare(
            "SELECT session_id, LENGTH(CAST(metadata_json AS BLOB)) \
             FROM session_head_metadata_cells \
             ORDER BY session_id, metadata_key, exact_value_digest",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let session_id: String = row.get(0)?;
            let bytes = measured_u64(row.get(1)?);
            let footprint = sessions.entry(session_id).or_default();
            pool.cell_rows += 1;
            match bytes {
                Some(bytes) => {
                    pool.cell_bytes = pool.cell_bytes.saturating_add(bytes);
                    footprint.head_metadata_bytes =
                        footprint.head_metadata_bytes.saturating_add(bytes);
                }
                None => {
                    footprint.unmeasured = true;
                    gaps.rows += 1;
                }
            }
        }
    }
    if table_exists(conn, "session_head_metadata_current")? {
        pool.current_rows = match measured_u64(Some(conn.query_row(
            "SELECT COUNT(*) FROM session_head_metadata_current",
            [],
            |row| row.get(0),
        )?)) {
            Some(count) => count,
            None => {
                gaps.rows += 1;
                0
            }
        };
    }
    if table_exists(conn, "session_head_metadata_states")? {
        let mut statement = conn.prepare(
            "SELECT session_id, LENGTH(CAST(identity_json AS BLOB)) \
             FROM session_head_metadata_states ORDER BY session_id, state_id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let session_id: String = row.get(0)?;
            let bytes = measured_u64(row.get(1)?);
            let footprint = sessions.entry(session_id).or_default();
            pool.state_rows += 1;
            match bytes {
                Some(bytes) => {
                    pool.state_identity_bytes = pool.state_identity_bytes.saturating_add(bytes);
                    footprint.head_metadata_bytes =
                        footprint.head_metadata_bytes.saturating_add(bytes);
                }
                None => {
                    footprint.unmeasured = true;
                    gaps.rows += 1;
                }
            }
        }
    }
    if table_exists(conn, "session_head_metadata_state_deltas")? {
        pool.delta_rows = match measured_u64(Some(conn.query_row(
            "SELECT COUNT(*) FROM session_head_metadata_state_deltas",
            [],
            |row| row.get(0),
        )?)) {
            Some(count) => count,
            None => {
                gaps.rows += 1;
                0
            }
        };
    }
    if table_exists(conn, "session_head_metadata_refs")? {
        let mut statement = conn.prepare(
            "SELECT owner, COUNT(*) FROM session_head_metadata_refs \
             GROUP BY owner ORDER BY owner",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let owner: String = row.get(0)?;
            let Some(count) = measured_u64(row.get(1)?) else {
                gaps.rows += 1;
                continue;
            };
            match owner.as_str() {
                "physical_head" => pool.physical_refs = count,
                "runtime_boundary" => pool.runtime_refs = count,
                _ => pool.unknown_refs = pool.unknown_refs.saturating_add(count),
            }
        }
    }
    if table_exists(conn, "session_head_metadata_head_lineage")? {
        pool.lineage_rows = match measured_u64(Some(conn.query_row(
            "SELECT COUNT(*) FROM session_head_metadata_head_lineage",
            [],
            |row| row.get(0),
        )?)) {
            Some(count) => count,
            None => {
                gaps.rows += 1;
                0
            }
        };
    }
    Ok(())
}

/// Blob rows: the stored byte length of every `sessions.session_json` (a
/// frozen archive when a head row exists), plus a raw-JSON measurement of
/// the live-vs-retained split for inline WholeBlob documents that have no
/// head row.
fn census_blob_rows(
    conn: &Connection,
    sessions: &mut BTreeMap<String, SessionFootprint>,
    archives: &mut FrozenArchiveCensus,
    gaps: &mut CensusGaps,
) -> Result<(), rusqlite::Error> {
    if !table_exists(conn, "sessions")? {
        return Ok(());
    }
    {
        let mut statement = conn.prepare(
            "SELECT session_id, LENGTH(CAST(session_json AS BLOB)) FROM sessions \
             ORDER BY session_id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let session_id: String = row.get(0)?;
            let bytes = measured_u64(row.get(1)?);
            let footprint = sessions.entry(session_id).or_default();
            let Some(bytes) = bytes else {
                footprint.unmeasured = true;
                gaps.rows += 1;
                continue;
            };
            footprint.blob_bytes = footprint.blob_bytes.saturating_add(bytes);
            if footprint.head_canonical() {
                archives.sessions += 1;
                archives.bytes = archives.bytes.saturating_add(bytes);
            }
        }
    }
    // Payloads are fetched only for the head-less rows: an archived document
    // is bytes to count, never bytes to read (the store does not read them
    // either).
    let sql = if table_exists(conn, "session_heads")? {
        "SELECT session_id, session_json FROM sessions \
         WHERE session_id NOT IN (SELECT session_id FROM session_heads) ORDER BY session_id"
    } else {
        "SELECT session_id, session_json FROM sessions ORDER BY session_id"
    };
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let session_id: String = row.get(0)?;
        let document: JsonColumnBytes = row.get(1)?;
        let footprint = sessions.entry(session_id).or_default();
        match measure_inline_document(&document.into_bytes()) {
            Some(measured) => {
                footprint.inline_live_bytes = measured.live_bytes;
                footprint.inline_graph_anchor_bytes = measured.graph_anchor_bytes;
                footprint.inline_graph_edge_bytes = measured.graph_edge_bytes;
                footprint.inline_graph_edges = measured.graph_edges;
                footprint.inline_graph_bytes = measured.graph_bytes;
                footprint.inline_released_revision_bodies = measured.released_revision_bodies;
                footprint.inline_released_commits = measured.released_commits;
                footprint.inline_graph_kind = measured.graph_kind;
            }
            None => {
                footprint.unmeasured = true;
                gaps.documents += 1;
            }
        }
    }
    archives.legacy_runtime_snapshots = legacy_runtime_snapshot_rows(conn)?;
    Ok(())
}

/// Rows in the retired `runtime_session_snapshots` table sharing this
/// database file. They are measured as frozen storage only; supported
/// runtimes no longer read them.
fn legacy_runtime_snapshot_rows(conn: &Connection) -> Result<u64, rusqlite::Error> {
    if !table_exists(conn, "runtime_session_snapshots")? {
        return Ok(0);
    }
    let mut statement = conn.prepare("SELECT COUNT(*) FROM runtime_session_snapshots")?;
    let count: i64 = statement.query_row([], |row| row.get(0))?;
    Ok(measured_u64(Some(count)).unwrap_or(0))
}

/// What an inline WholeBlob document holds, measured from its raw stored
/// bytes.
struct InlineDocumentMeasurement {
    live_bytes: u64,
    graph_anchor_bytes: u64,
    graph_edge_bytes: u64,
    graph_edges: u64,
    graph_bytes: u64,
    released_revision_bodies: u64,
    released_commits: u64,
    graph_kind: Option<InlineGraphKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineGraphKind {
    Compact,
    Released0810,
}

/// Doctor's raw lens over a persisted session document: the live transcript
/// array and the inline transcript-history graph.
///
/// Deliberately not a typed [`meerkat_core::Session`] decode — the census must
/// measure the bytes the disk holds (a typed round-trip would report the bytes
/// this binary *would* write), and it must keep measuring documents that a
/// future envelope change makes typed-undecodable.
#[derive(Deserialize)]
struct InlineDocumentLens<'a> {
    #[serde(borrow)]
    messages: &'a serde_json::value::RawValue,
    #[serde(borrow, default)]
    metadata: BTreeMap<String, &'a serde_json::value::RawValue>,
}

/// Format discriminator only. Doctor selects an exact physical lens from this
/// field; an unknown current/future value is unmeasurable rather than silently
/// interpreted as released 0.8.10.
#[derive(Deserialize)]
struct InlineHistoryFormatLens<'a> {
    #[serde(borrow, default)]
    format: Option<&'a str>,
}

/// Current compact graph lens. Raw references preserve exact stored byte
/// lengths and never materialize the anchor or edge message bodies.
#[derive(Deserialize)]
struct CompactInlineHistoryLens<'a> {
    #[serde(borrow)]
    anchor: &'a serde_json::value::RawValue,
    #[serde(borrow)]
    edges: Vec<&'a serde_json::value::RawValue>,
}

/// Exact released-0.8.10 graph lens. This is the only supported predecessor
/// format and is counted as the full-body representation it physically is.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Released0810InlineHistoryLens<'a> {
    #[serde(borrow, rename = "head")]
    _released_head: &'a str,
    #[serde(borrow, default, rename = "commits")]
    released_commit_rows: Vec<&'a serde_json::value::RawValue>,
    #[serde(borrow, default, rename = "revisions")]
    released_revision_rows: Vec<&'a serde_json::value::RawValue>,
    #[serde(default, rename = "digest_format")]
    _released_digest_format: u32,
    #[serde(borrow, default, rename = "replay_cursor")]
    _released_replay_cursor: Option<&'a serde_json::value::RawValue>,
}

fn measure_inline_history(
    value: &serde_json::value::RawValue,
) -> Option<InlineDocumentMeasurement> {
    let raw = value.get();
    let format: InlineHistoryFormatLens<'_> = serde_json::from_str(raw).ok()?;
    match format.format {
        Some(TRANSCRIPT_HISTORY_FORMAT_CURRENT) => {
            let graph: CompactInlineHistoryLens<'_> = serde_json::from_str(raw).ok()?;
            Some(InlineDocumentMeasurement {
                live_bytes: 0,
                graph_anchor_bytes: graph.anchor.get().len() as u64,
                graph_edge_bytes: graph.edges.iter().map(|edge| edge.get().len() as u64).sum(),
                graph_edges: graph.edges.len() as u64,
                graph_bytes: raw.len() as u64,
                released_revision_bodies: 0,
                released_commits: 0,
                graph_kind: Some(InlineGraphKind::Compact),
            })
        }
        Some(_) => None,
        None => {
            let graph: Released0810InlineHistoryLens<'_> = serde_json::from_str(raw).ok()?;
            if graph.released_commit_rows.is_empty() || graph.released_revision_rows.is_empty() {
                return None;
            }
            Some(InlineDocumentMeasurement {
                live_bytes: 0,
                graph_anchor_bytes: 0,
                graph_edge_bytes: 0,
                graph_edges: 0,
                graph_bytes: raw.len() as u64,
                released_revision_bodies: graph.released_revision_rows.len() as u64,
                released_commits: graph.released_commit_rows.len() as u64,
                graph_kind: Some(InlineGraphKind::Released0810),
            })
        }
    }
}

/// `None` when the document cannot be measured (the caller then reports it
/// unknown instead of assuming a healthy split).
fn measure_inline_document(document: &[u8]) -> Option<InlineDocumentMeasurement> {
    let lens: InlineDocumentLens<'_> = serde_json::from_slice(document).ok()?;
    let mut measured = match lens.metadata.get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY) {
        Some(value) => measure_inline_history(value)?,
        None => InlineDocumentMeasurement {
            live_bytes: 0,
            graph_anchor_bytes: 0,
            graph_edge_bytes: 0,
            graph_edges: 0,
            graph_bytes: 0,
            released_revision_bodies: 0,
            released_commits: 0,
            graph_kind: None,
        },
    };
    measured.live_bytes = lens.messages.get().len() as u64;
    Some(measured)
}

/// Per-session footprint findings, worst reclaimable first, plus one
/// database-wide summary whenever at least one session qualifies.
fn report_oversized_sessions(
    sessions: &BTreeMap<String, SessionFootprint>,
    gaps: &CensusGaps,
    db_path: &Path,
    realm: &str,
    diagnosis: &mut StorageDiagnosis,
) {
    let mut oversized: Vec<(&String, &SessionFootprint)> = sessions
        .iter()
        .filter(|(_, footprint)| footprint.is_oversized())
        .collect();
    if oversized.is_empty() {
        return;
    }
    // The cap has to spend its lines on the biggest wins: a realm where
    // thousands of sessions qualify still needs a usable report. Ties break
    // by session id so the report is stable across runs.
    oversized.sort_by_key(|(id, footprint)| (Reverse(footprint.reclaimable_bytes()), *id));

    for (session_id, footprint) in oversized.iter().take(TRANSCRIPT_HISTORY_REPORT_CAP) {
        let retained = if footprint.head_canonical() {
            format!(
                "{} retained revision strand(s) and {} rewrite commit(s) in \
                 session_strand_messages + session_rewrites",
                footprint.retained_revisions(),
                footprint.rewrite_commits()
            )
        } else {
            footprint.inline_graph_description()
        };
        diagnosis.findings.push(
            StorageFinding::new(
                FindingSeverity::Warning,
                FINDING_TRANSCRIPT_HISTORY_OVERSIZED,
                format!(
                    "session {session_id} stores {} of durable transcript for {} of live \
                     transcript, ratio {}: {retained}; {} lies outside the serialized live \
                     transcript (history plus required envelope/authority bytes)",
                    format_bytes(footprint.document_bytes()),
                    format_bytes(footprint.live_bytes()),
                    format_ratio(footprint.document_bytes(), footprint.live_bytes()),
                    format_bytes(footprint.reclaimable_bytes()),
                ),
            )
            .with_path(db_path.to_path_buf())
            .with_realm(realm),
        );
    }

    // Head-less strand sessions are excluded exactly like unmeasured ones:
    // folding their bytes in with zero live would publish a database-wide
    // ratio built on a guess (the exclusion note carries them instead).
    let measured = sessions
        .values()
        .filter(|f| !f.unmeasured && !f.split_unclassifiable());
    let (measured_sessions, document_bytes, live_bytes) =
        measured.fold((0u64, 0u64, 0u64), |(count, documents, live), footprint| {
            (
                count + 1,
                documents.saturating_add(footprint.document_bytes()),
                live.saturating_add(footprint.live_bytes()),
            )
        });
    let overflow = oversized
        .len()
        .saturating_sub(TRANSCRIPT_HISTORY_REPORT_CAP);
    let overflow_note = if overflow > 0 {
        format!("; {overflow} further session(s) over the threshold are not listed individually")
    } else {
        String::new()
    };
    diagnosis.findings.push(
        StorageFinding::new(
            FindingSeverity::Warning,
            FINDING_TRANSCRIPT_HISTORY_OVERSIZED,
            format!(
                "{} of {measured_sessions} measured session(s) exceed the \
                 {TRANSCRIPT_HISTORY_RATIO_WARN:.1}x durable-to-live threshold; database-wide {} \
                 durable for {} of live transcript, ratio {}, {} beyond the serialized live \
                 transcript{overflow_note}{}",
                oversized.len(),
                format_bytes(document_bytes),
                format_bytes(live_bytes),
                format_ratio(document_bytes, live_bytes),
                format_bytes(document_bytes.saturating_sub(live_bytes)),
                gaps.exclusion_note(),
            ),
        )
        .with_path(db_path.to_path_buf())
        .with_realm(realm),
    );
}

/// Aggregate raw inline graph shape. This is emitted even for healthy compact
/// graphs: otherwise doctor would remain silent precisely when delta
/// retention is working and operators could not distinguish it from an
/// unmeasured pool.
fn report_inline_graph_pool(
    sessions: &BTreeMap<String, SessionFootprint>,
    gaps: &CensusGaps,
    db_path: &Path,
    realm: &str,
    diagnosis: &mut StorageDiagnosis,
) {
    let mut compact_sessions = 0u64;
    let mut compact_anchor_bytes = 0u64;
    let mut compact_edges = 0u64;
    let mut compact_edge_bytes = 0u64;
    let mut compact_graph_bytes = 0u64;
    let mut released_sessions = 0u64;
    let mut released_bodies = 0u64;
    let mut released_commits = 0u64;
    let mut released_graph_bytes = 0u64;

    for footprint in sessions.values() {
        match footprint.inline_graph_kind {
            Some(InlineGraphKind::Compact) => {
                compact_sessions += 1;
                compact_anchor_bytes =
                    compact_anchor_bytes.saturating_add(footprint.inline_graph_anchor_bytes);
                compact_edges = compact_edges.saturating_add(footprint.inline_graph_edges);
                compact_edge_bytes =
                    compact_edge_bytes.saturating_add(footprint.inline_graph_edge_bytes);
                compact_graph_bytes =
                    compact_graph_bytes.saturating_add(footprint.inline_graph_bytes);
            }
            Some(InlineGraphKind::Released0810) => {
                released_sessions += 1;
                released_bodies =
                    released_bodies.saturating_add(footprint.inline_released_revision_bodies);
                released_commits =
                    released_commits.saturating_add(footprint.inline_released_commits);
                released_graph_bytes =
                    released_graph_bytes.saturating_add(footprint.inline_graph_bytes);
            }
            None => {}
        }
    }
    if compact_sessions == 0 && released_sessions == 0 {
        return;
    }

    let mut clauses = Vec::new();
    if compact_sessions > 0 {
        clauses.push(format!(
            "{compact_sessions} current compact inline graph(s): one anchor each / {} total, \
             {compact_edges} occurrence edge(s) / {}, {} total graph bytes including rolling \
             witnesses; no full historical revision bodies were materialized by doctor",
            format_bytes(compact_anchor_bytes),
            format_bytes(compact_edge_bytes),
            format_bytes(compact_graph_bytes),
        ));
    }
    if released_sessions > 0 {
        clauses.push(format!(
            "{released_sessions} exact released-0.8.10 inline graph(s): \
             {released_bodies} full revision bod(ies), {released_commits} rewrite commit(s), {} \
             total graph bytes",
            format_bytes(released_graph_bytes),
        ));
    }
    let message = format!(
        "`sessions.session_json` transcript-history footprint: {}{}",
        clauses.join("; "),
        gaps.exclusion_note(),
    );
    diagnosis.findings.push(
        StorageFinding::new(
            FindingSeverity::Info,
            FINDING_INLINE_TRANSCRIPT_GRAPH_FOOTPRINT,
            message,
        )
        .with_path(db_path.to_path_buf())
        .with_realm(realm),
    );
    if released_sessions > 0 {
        diagnosis.findings.push(
            StorageFinding::new(
                FindingSeverity::Warning,
                FINDING_RELEASED_TRANSCRIPT_GRAPH_SEMANTIC_VERIFICATION_DEFERRED,
                format!(
                    "semantic_verification_deferred=true for {released_sessions} exact \
                     released-0.8.10 inline graph(s): bounded doctor measured the frozen physical \
                     shape and {}, but did not normalize or retain arbitrary-base historical \
                     revision bodies; exact semantic proof runs at the one-time persisted-session \
                     ingress/migration boundary",
                    format_bytes(released_graph_bytes),
                ),
            )
            .with_path(db_path.to_path_buf())
            .with_realm(realm),
        );
    }
}

/// The strand pool, reported whenever it holds a row or a supersession link
/// — a healthy pool nobody can see is how this class of defect stays
/// invisible.
fn report_strand_pool(
    pool: &StrandPoolCensus,
    gaps: &CensusGaps,
    db_path: &Path,
    realm: &str,
    diagnosis: &mut StorageDiagnosis,
) {
    if pool.rows == 0 && pool.links == 0 {
        return;
    }
    let reclaimable = pool.retained_bytes.saturating_add(pool.unclassified_bytes);
    let duplicated = reclaimable >= STORAGE_CENSUS_RECLAIMABLE_FLOOR_BYTES
        && (pool.live_bytes == 0
            || pool.bytes as f64 / pool.live_bytes as f64 >= STRAND_DUPLICATION_RATIO_WARN);
    let severity = if duplicated {
        FindingSeverity::Warning
    } else {
        FindingSeverity::Info
    };
    let mut message = format!(
        "`session_strand_messages` holds {} row(s) / {} across {} session(s): {} row(s) / {} are \
         live head-strand prefixes and {} row(s) / {} are retained non-live revisions, \
         pool-to-live ratio {}",
        pool.rows,
        format_bytes(pool.bytes),
        pool.sessions,
        pool.live_rows,
        format_bytes(pool.live_bytes),
        pool.retained_rows,
        format_bytes(pool.retained_bytes),
        format_ratio(pool.bytes, pool.live_bytes),
    );
    if pool.unclassified_rows > 0 {
        message.push_str(&format!(
            "; {} row(s) / {} belong to sessions with no usable `session_heads` row and could not \
             be classified live-or-retained",
            pool.unclassified_rows,
            format_bytes(pool.unclassified_bytes),
        ));
    }
    if pool.links > 0 {
        message.push_str(&format!(
            "; {} `session_strand_links` supersession row(s) keep superseded strands down to \
             their divergent span (counted, not byte-measured: the row carries no payload)",
            pool.links,
        ));
    }
    message.push_str(&gaps.exclusion_note());
    diagnosis.findings.push(
        StorageFinding::new(severity, FINDING_STRAND_DUPLICATION_RECLAIMABLE, message)
            .with_path(db_path.to_path_buf())
            .with_realm(realm),
    );
}

/// Frozen blob archives: bytes held for sessions whose canonical
/// representation is the head row.
fn report_frozen_archives(
    archives: &FrozenArchiveCensus,
    gaps: &CensusGaps,
    db_path: &Path,
    realm: &str,
    diagnosis: &mut StorageDiagnosis,
) {
    if archives.sessions == 0 {
        return;
    }
    let severity = if archives.bytes >= STORAGE_CENSUS_RECLAIMABLE_FLOOR_BYTES {
        FindingSeverity::Warning
    } else {
        FindingSeverity::Info
    };
    // The claim is verified, not inherited: every SqliteSessionStore read
    // (load, load_meta, load_head, load_messages, load_rewrites,
    // load_rewrite_commits, and each save/delete guard) resolves the head row
    // first and only falls back to the blob row when there is none, and
    // `list` excludes blob rows that have a head row outright.
    let mut message = format!(
        "{} frozen `sessions.session_json` row(s) hold {} for session(s) that already have a \
         `session_heads` row; every SqliteSessionStore read resolves the head row first and \
         `list` excludes blob rows that have one, so these bytes are never read again \
         (meerkat-store/src/sqlite_store.rs: \"The blob row is left untouched as a frozen archive \
         and is never read again once the head row exists\")",
        archives.sessions,
        format_bytes(archives.bytes),
    );
    if archives.legacy_runtime_snapshots > 0 {
        message.push_str(&format!(
            "; this database also holds {} retired `runtime_session_snapshots` row(s), which \
             supported runtimes no longer read",
            archives.legacy_runtime_snapshots,
        ));
    }
    message.push_str(&gaps.exclusion_note());
    diagnosis.findings.push(
        StorageFinding::new(severity, FINDING_FROZEN_BLOB_ARCHIVE_RECLAIMABLE, message)
            .with_path(db_path.to_path_buf())
            .with_realm(realm),
    );
}

/// Metadata cell/state/ref visibility. JSON byte totals are exact without
/// hydrating any payload; compact current, delta, owner, and lineage records
/// are reported as row counts.
fn report_head_metadata_pool(
    pool: &HeadMetadataPoolCensus,
    gaps: &CensusGaps,
    db_path: &Path,
    realm: &str,
    diagnosis: &mut StorageDiagnosis,
) {
    if pool.cell_rows == 0
        && pool.current_rows == 0
        && pool.state_rows == 0
        && pool.delta_rows == 0
        && pool.physical_refs == 0
        && pool.runtime_refs == 0
        && pool.unknown_refs == 0
        && pool.lineage_rows == 0
    {
        return;
    }
    let mut message = format!(
        "`session_head_metadata_cells` holds {} immutable cell row(s) / {}; \
         `session_head_metadata_current` holds {} current physical cell reference row(s); \
         `session_head_metadata_states` holds {} immutable state row(s) / {}; \
         `session_head_metadata_state_deltas` holds {} per-state delta row(s); \
         `session_head_metadata_refs` holds {} physical-head and {} runtime-boundary exact \
         owner reference row(s); `session_head_metadata_head_lineage` holds {} head-token \
         lineage row(s) (compact rows counted, not byte-measured: they carry no JSON payload)",
        pool.cell_rows,
        format_bytes(pool.cell_bytes),
        pool.current_rows,
        pool.state_rows,
        format_bytes(pool.state_identity_bytes),
        pool.delta_rows,
        pool.physical_refs,
        pool.runtime_refs,
        pool.lineage_rows,
    );
    if pool.unknown_refs > 0 {
        message.push_str(&format!(
            "; {} reference row(s) have an owner this binary does not understand",
            pool.unknown_refs,
        ));
    }
    message.push_str(&gaps.exclusion_note());
    diagnosis.findings.push(
        StorageFinding::new(
            FindingSeverity::Info,
            FINDING_HEAD_METADATA_SIDECAR,
            message,
        )
        .with_path(db_path.to_path_buf())
        .with_realm(realm),
    );
}

/// The census's own honesty finding: what it could not measure.
fn report_census_gaps(
    gaps: &CensusGaps,
    db_path: &Path,
    realm: &str,
    diagnosis: &mut StorageDiagnosis,
) {
    if gaps.is_empty() {
        return;
    }
    let headless = if gaps.headless_strand_sessions == 0 {
        String::new()
    } else {
        format!(
            ", and could not classify {} session(s) holding strand rows with no \
             `session_heads` row (head save not yet landed; live-or-retained split unknowable)",
            gaps.headless_strand_sessions,
        )
    };
    let pools = if gaps.pools.is_empty() {
        String::new()
    } else {
        format!(" and could not query {} at all", gaps.pools.join(", "))
    };
    diagnosis.findings.push(
        StorageFinding::new(
            FindingSeverity::Warning,
            FINDING_STORAGE_CENSUS_UNMEASURED,
            format!(
                "storage census could not measure {} durable row(s) and {} session \
                 document(s){headless}{pools}; the footprint findings exclude them, so this \
                 database's storage footprint is UNKNOWN rather than certified healthy",
                gaps.rows, gaps.documents,
            ),
        )
        .with_path(db_path.to_path_buf())
        .with_realm(realm),
    );
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
    use meerkat_core::{
        ImageData, Session, TranscriptRewriteReason, TranscriptRewriteSelection, UserMessage,
    };

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

    // The head-canonical session tables, exactly as `SqliteSessionStore`
    // creates them (see `sqlite_store::CREATE_SESSION_*_SQL`).
    const SESSION_HEADS_DDL: &str = "CREATE TABLE session_heads (
        session_id TEXT PRIMARY KEY,
        version INTEGER NOT NULL,
        strand TEXT NOT NULL,
        head_revision TEXT NOT NULL,
        message_count INTEGER NOT NULL,
        rewrite_count INTEGER NOT NULL,
        total_tokens INTEGER NOT NULL,
        created_at_ms INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL,
        metadata_json TEXT NOT NULL,
        head_json BLOB NOT NULL,
        cas_token TEXT NOT NULL
    )";

    const SESSION_STRAND_MESSAGES_DDL: &str = "CREATE TABLE session_strand_messages (
        session_id TEXT NOT NULL,
        strand TEXT NOT NULL,
        seq INTEGER NOT NULL,
        message_json BLOB NOT NULL,
        created_at_ms INTEGER NOT NULL,
        PRIMARY KEY (session_id, strand, seq)
    )";

    const SESSION_STRAND_LINKS_DDL: &str = "CREATE TABLE session_strand_links (
        session_id TEXT NOT NULL,
        strand TEXT NOT NULL,
        successor TEXT NOT NULL,
        strand_len INTEGER NOT NULL,
        splice_start INTEGER NOT NULL,
        splice_end INTEGER NOT NULL,
        successor_end INTEGER NOT NULL,
        created_at_ms INTEGER NOT NULL,
        PRIMARY KEY (session_id, strand)
    )";

    const SESSION_REWRITES_DDL: &str = "CREATE TABLE session_rewrites (
        session_id TEXT NOT NULL,
        rewrite_idx INTEGER NOT NULL,
        parent_strand TEXT NOT NULL,
        parent_len INTEGER NOT NULL,
        strand TEXT NOT NULL,
        strand_len INTEGER NOT NULL,
        commit_json BLOB NOT NULL,
        created_at_ms INTEGER NOT NULL,
        PRIMARY KEY (session_id, rewrite_idx)
    )";

    /// A sqlite session database carrying the full head-canonical schema.
    fn head_canonical_db(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(SESSIONS_DDL).unwrap();
        conn.execute_batch(SESSION_HEADS_DDL).unwrap();
        conn.execute_batch(SESSION_STRAND_MESSAGES_DDL).unwrap();
        conn.execute_batch(SESSION_STRAND_LINKS_DDL).unwrap();
        conn.execute_batch(SESSION_REWRITES_DDL).unwrap();
        conn
    }

    /// One persisted transcript message carrying `payload` bytes of text.
    fn strand_message(payload: usize) -> Vec<u8> {
        serde_json::to_vec(&Message::User(UserMessage::text("x".repeat(payload)))).unwrap()
    }

    /// Insert a head row; returns the durable head bytes the census must
    /// charge to the session (`head_json` + `metadata_json`).
    fn insert_head(conn: &Connection, id: &SessionId, strand: &str, message_count: u64) -> u64 {
        let head_json = serde_json::json!({
            "id": id.to_string(),
            "strand": strand,
            "message_count": message_count,
        })
        .to_string();
        let metadata_json = "{}";
        conn.execute(
            "INSERT INTO session_heads (session_id, version, strand, head_revision, \
             message_count, rewrite_count, total_tokens, created_at_ms, updated_at_ms, \
             metadata_json, head_json, cas_token) \
             VALUES (?1, 1, ?2, 'digest', ?3, 0, 0, 0, 0, ?4, ?5, 'cas')",
            rusqlite::params![
                id.to_string(),
                strand,
                message_count as i64,
                metadata_json,
                head_json.as_bytes(),
            ],
        )
        .unwrap();
        (head_json.len() + metadata_json.len()) as u64
    }

    fn insert_strand_row(
        conn: &Connection,
        id: &SessionId,
        strand: &str,
        seq: u64,
        message_json: &[u8],
    ) {
        conn.execute(
            "INSERT INTO session_strand_messages (session_id, strand, seq, message_json, \
             created_at_ms) VALUES (?1, ?2, ?3, ?4, 0)",
            rusqlite::params![id.to_string(), strand, seq as i64, message_json],
        )
        .unwrap();
    }

    fn finding<'a>(diagnosis: &'a StorageDiagnosis, code: &str) -> Option<&'a StorageFinding> {
        diagnosis.findings.iter().find(|f| f.code == code)
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
    async fn oversized_transcript_history_is_measured_per_session_and_summarized() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "bloat", "sqlite");
        let db_path = realm_dir.join("sessions.sqlite3");
        let id = Session::new().id().clone();
        let message = strand_message(64 * 1024);
        let head_bytes;
        {
            let conn = head_canonical_db(&db_path);
            head_bytes = insert_head(&conn, &id, "root", 4);
            for seq in 0..4 {
                insert_strand_row(&conn, &id, "root", seq, &message);
            }
            // Six retained revisions, each a full copy of the four-message
            // transcript: the shape per-rewrite full-transcript retention
            // produces.
            for revision in 0..6u64 {
                for seq in 0..4 {
                    insert_strand_row(&conn, &id, &format!("rewrite:{revision}"), seq, &message);
                }
            }
        }
        let before = std::fs::read(&db_path).unwrap();

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;

        let live_bytes = 4 * message.len() as u64;
        let document_bytes = head_bytes + 28 * message.len() as u64;
        let per_session = diagnosis
            .findings
            .iter()
            .find(|f| {
                f.code == FINDING_TRANSCRIPT_HISTORY_OVERSIZED
                    && f.message.contains(&id.to_string())
            })
            .expect("per-session footprint finding");
        assert_eq!(per_session.severity, FindingSeverity::Warning);
        assert_eq!(per_session.realm.as_deref(), Some("bloat"));
        assert_eq!(per_session.path.as_deref(), Some(db_path.as_path()));
        assert!(
            per_session.message.contains(&format!(
                "{:.1}x",
                document_bytes as f64 / live_bytes as f64
            )),
            "{}",
            per_session.message
        );
        assert!(
            per_session
                .message
                .contains("6 retained revision strand(s) and 0 rewrite commit(s)"),
            "{}",
            per_session.message
        );
        assert!(
            per_session
                .message
                .contains(&format_bytes(document_bytes - live_bytes)),
            "{}",
            per_session.message
        );

        let summary = diagnosis
            .findings
            .iter()
            .find(|f| {
                f.code == FINDING_TRANSCRIPT_HISTORY_OVERSIZED
                    && f.message.contains("measured session(s) exceed")
            })
            .expect("database-wide summary finding");
        assert!(
            summary
                .message
                .contains("1 of 1 measured session(s) exceed the 4.0x"),
            "{}",
            summary.message
        );
        assert!(
            !summary.message.contains("partial census"),
            "{}",
            summary.message
        );

        // The duplicated copies are visible in the strand pool too.
        let pool = finding(&diagnosis, FINDING_STRAND_DUPLICATION_RECLAIMABLE)
            .expect("strand pool finding");
        assert_eq!(pool.severity, FindingSeverity::Warning);
        assert!(pool.message.contains("holds 28 row(s)"), "{}", pool.message);
        let live_clause = format!(
            "4 row(s) / {} are live head-strand prefixes",
            format_bytes(live_bytes)
        );
        let retained_clause = format!(
            "24 row(s) / {} are retained non-live revisions",
            format_bytes(24 * message.len() as u64)
        );
        assert!(pool.message.contains(&live_clause), "{}", pool.message);
        assert!(pool.message.contains(&retained_clause), "{}", pool.message);
        assert!(
            pool.message.contains("pool-to-live ratio 7.0x"),
            "{}",
            pool.message
        );

        // Nothing was archived and nothing was unmeasurable.
        assert!(!codes(&diagnosis).contains(&FINDING_FROZEN_BLOB_ARCHIVE_RECLAIMABLE));
        assert!(!codes(&diagnosis).contains(&FINDING_STORAGE_CENSUS_UNMEASURED));
        assert!(!diagnosis.has_errors(), "{diagnosis:?}");
        // Read-only: the census measures, it never writes.
        assert_eq!(std::fs::read(&db_path).unwrap(), before);
    }

    #[tokio::test]
    async fn healthy_strand_pool_is_reported_without_a_footprint_warning() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "healthy", "sqlite");
        let db_path = realm_dir.join("sessions.sqlite3");
        let id = Session::new().id().clone();
        let message = strand_message(1024);
        {
            let conn = head_canonical_db(&db_path);
            insert_head(&conn, &id, "root", 4);
            for seq in 0..4 {
                insert_strand_row(&conn, &id, "root", seq, &message);
            }
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;

        assert!(
            !codes(&diagnosis).contains(&FINDING_TRANSCRIPT_HISTORY_OVERSIZED),
            "{diagnosis:?}"
        );
        assert!(!codes(&diagnosis).contains(&FINDING_FROZEN_BLOB_ARCHIVE_RECLAIMABLE));
        assert!(!codes(&diagnosis).contains(&FINDING_STORAGE_CENSUS_UNMEASURED));
        // A healthy pool is still measured out loud: an unmeasured pool is
        // exactly how this class of defect stays invisible.
        let pool = finding(&diagnosis, FINDING_STRAND_DUPLICATION_RECLAIMABLE)
            .expect("strand pool finding even when healthy");
        assert_eq!(pool.severity, FindingSeverity::Info);
        assert!(pool.message.contains("holds 4 row(s)"), "{}", pool.message);
        assert!(
            pool.message
                .contains("0 row(s) / 0 B are retained non-live revisions"),
            "{}",
            pool.message
        );
        assert!(
            pool.message.contains("pool-to-live ratio 1.0x"),
            "{}",
            pool.message
        );
        assert!(!pool.message.contains("partial census"), "{}", pool.message);
        assert!(!diagnosis.has_errors(), "{diagnosis:?}");
    }

    #[tokio::test]
    async fn frozen_blob_archives_are_counted_only_behind_a_head_row() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "archive", "sqlite");
        let db_path = realm_dir.join("sessions.sqlite3");

        let mut archived = Session::new();
        archived.push(Message::User(UserMessage::text("a".repeat(1536 * 1024))));
        let archived_bytes = serde_json::to_vec(&archived).unwrap().len() as u64;
        let mut legacy = Session::new();
        legacy.push(Message::User(UserMessage::text("b".repeat(640 * 1024))));
        let legacy_bytes = serde_json::to_vec(&legacy).unwrap().len() as u64;
        let live = strand_message(1024);
        {
            let conn = head_canonical_db(&db_path);
            // Migrated session: head row canonical, blob row frozen.
            insert_session(&conn, &archived);
            insert_head(&conn, archived.id(), "root", 1);
            insert_strand_row(&conn, archived.id(), "root", 0, &live);
            // Legacy session: blob row is still the canonical document.
            insert_session(&conn, &legacy);
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;

        let archives = finding(&diagnosis, FINDING_FROZEN_BLOB_ARCHIVE_RECLAIMABLE)
            .expect("frozen archive finding");
        assert_eq!(archives.severity, FindingSeverity::Warning);
        assert_eq!(archives.realm.as_deref(), Some("archive"));
        assert!(
            archives.message.starts_with("1 frozen"),
            "only the head-backed row is an archive: {}",
            archives.message
        );
        assert!(
            archives.message.contains(&format_bytes(archived_bytes)),
            "{}",
            archives.message
        );
        assert!(
            !archives
                .message
                .contains(&format_bytes(archived_bytes + legacy_bytes)),
            "the legacy document must not be counted as an archive: {}",
            archives.message
        );
        assert!(
            archives
                .message
                .contains("never read again once the head row exists"),
            "{}",
            archives.message
        );

        // Archive bytes are durable bytes: they count toward the session's
        // footprint, and the legacy document (all live) does not warn.
        let oversized: Vec<&StorageFinding> = diagnosis
            .findings
            .iter()
            .filter(|f| f.code == FINDING_TRANSCRIPT_HISTORY_OVERSIZED)
            .collect();
        assert!(
            oversized
                .iter()
                .any(|f| f.message.contains(&archived.id().to_string())),
            "{oversized:?}"
        );
        assert!(
            !oversized
                .iter()
                .any(|f| f.message.contains(&legacy.id().to_string())),
            "{oversized:?}"
        );
    }

    #[tokio::test]
    async fn compact_inline_transcript_history_is_measured_from_raw_anchor_and_edges() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "inline", "sqlite");
        let db_path = realm_dir.join("sessions.sqlite3");

        // A document whose retained history is genuinely heavy. Under delta
        // retention a rewrite costs its EDIT, so churning a small message
        // over a large transcript no longer retains much of anything — the
        // fixture has to replace large content with different large content
        // for the retained spans themselves to be worth reclaiming. That is
        // also the shape the census exists to flag now: real content churn,
        // not a small repeated replacement that delta encoding bounds.
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("c".repeat(512 * 1024))));
        session.push(Message::User(UserMessage::text("second turn")));
        for revision in 0..8 {
            let replacement = Message::User(UserMessage::text(
                format!("edited turn {revision} ").repeat(24 * 1024),
            ));
            session
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    vec![replacement],
                    TranscriptRewriteReason::new("doctor-census-fixture"),
                    None,
                    None,
                )
                .expect("rewrite commits");
        }
        let document = serde_json::to_vec(&session).unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(&document).unwrap();
        let history = &decoded["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY];
        assert_eq!(
            history["format"].as_str(),
            Some(TRANSCRIPT_HISTORY_FORMAT_CURRENT)
        );
        let expected_anchor_bytes = serde_json::to_vec(&history["anchor"]).unwrap().len() as u64;
        let expected_edges = history["edges"].as_array().unwrap();
        let expected_edge_bytes = expected_edges
            .iter()
            .map(|edge| serde_json::to_vec(edge).unwrap().len() as u64)
            .sum::<u64>();
        let expected_graph_bytes = serde_json::to_vec(history).unwrap().len() as u64;
        let live_bytes = serde_json::to_string(&decoded["messages"]).unwrap().len() as u64;
        let document_bytes = document.len() as u64;
        // Guards: a fixture that stopped reproducing inline retention must
        // fail here rather than pass vacuously.
        assert_eq!(expected_edges.len(), 8);
        assert!(
            document_bytes - live_bytes >= STORAGE_CENSUS_RECLAIMABLE_FLOOR_BYTES,
            "{document_bytes} - {live_bytes}"
        );
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(SESSIONS_DDL).unwrap();
            conn.execute(
                "INSERT INTO sessions (session_id, created_at_ms, updated_at_ms, message_count, \
                 total_tokens, metadata_json, session_json) VALUES (?1, 0, 0, ?2, 0, ?3, ?4)",
                rusqlite::params![
                    session.id().to_string(),
                    session.messages().len() as i64,
                    serde_json::to_string(&decoded["metadata"]).unwrap(),
                    document.as_slice(),
                ],
            )
            .unwrap();
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;

        let per_session = finding(&diagnosis, FINDING_TRANSCRIPT_HISTORY_OVERSIZED)
            .expect("inline footprint finding");
        assert_eq!(per_session.severity, FindingSeverity::Warning);
        assert!(
            per_session
                .message
                .contains("compact transcript graph inline in sessions.session_json"),
            "{}",
            per_session.message
        );
        assert!(
            per_session.message.contains(&format!(
                "1 anchor / {}, {} occurrence edge(s) / {}, {} total graph bytes",
                format_bytes(expected_anchor_bytes),
                expected_edges.len(),
                format_bytes(expected_edge_bytes),
                format_bytes(expected_graph_bytes),
            )),
            "{}",
            per_session.message
        );
        assert!(
            !per_session.message.contains("revision bod"),
            "compact edges must not be reported as full revision bodies: {}",
            per_session.message
        );
        let graph_pool = finding(&diagnosis, FINDING_INLINE_TRANSCRIPT_GRAPH_FOOTPRINT)
            .expect("inline compact graph footprint finding");
        assert_eq!(graph_pool.severity, FindingSeverity::Info);
        assert!(
            graph_pool.message.contains(&format!(
                "1 current compact inline graph(s): one anchor each / {} total, {} occurrence \
                 edge(s) / {}, {} total graph bytes",
                format_bytes(expected_anchor_bytes),
                expected_edges.len(),
                format_bytes(expected_edge_bytes),
                format_bytes(expected_graph_bytes),
            )),
            "{}",
            graph_pool.message
        );
        assert!(
            graph_pool
                .message
                .contains("no full historical revision bodies were materialized by doctor"),
            "{}",
            graph_pool.message
        );
        assert!(
            per_session.message.contains(&format!(
                "{:.1}x",
                document_bytes as f64 / live_bytes as f64
            )),
            "{}",
            per_session.message
        );
        assert!(
            per_session.message.contains(&format_bytes(document_bytes)),
            "{}",
            per_session.message
        );
        // No strand rows and no head rows exist here: the report must not
        // imply those pools were censused.
        assert!(!codes(&diagnosis).contains(&FINDING_STRAND_DUPLICATION_RECLAIMABLE));
        assert!(!codes(&diagnosis).contains(&FINDING_FROZEN_BLOB_ARCHIVE_RECLAIMABLE));
        assert!(!codes(&diagnosis).contains(&FINDING_STORAGE_CENSUS_UNMEASURED));
        assert!(!diagnosis.has_errors(), "{diagnosis:?}");
    }

    #[test]
    fn released_0_8_10_inline_history_is_classified_as_full_body_storage() {
        let document = br#"{
            "messages": [{"role":"user","content":[{"type":"text","text":"live"}]}],
            "metadata": {
                "session_transcript_history_state_v1": {
                    "head": "sha256:head",
                    "commits": [{"rewrite_generation":1}],
                    "revisions": [
                        {"revision":"sha256:parent","messages":[]},
                        {"revision":"sha256:head","messages":[]}
                    ],
                    "digest_format": 2
                }
            }
        }"#;
        let measured = measure_inline_document(document).expect("released graph must measure");
        assert_eq!(measured.graph_kind, Some(InlineGraphKind::Released0810));
        assert_eq!(measured.released_commits, 1);
        assert_eq!(measured.released_revision_bodies, 2);
        assert_eq!(measured.graph_edges, 0);
        assert_eq!(measured.graph_anchor_bytes, 0);
        assert!(measured.graph_bytes > 0);

        let mut footprint = SessionFootprint::default();
        footprint.inline_graph_kind = measured.graph_kind;
        footprint.inline_graph_bytes = measured.graph_bytes;
        footprint.inline_released_commits = measured.released_commits;
        footprint.inline_released_revision_bodies = measured.released_revision_bodies;
        let sessions = BTreeMap::from([("released".to_string(), footprint)]);
        let mut diagnosis = StorageDiagnosis::default();
        report_inline_graph_pool(
            &sessions,
            &CensusGaps::default(),
            Path::new("sessions.sqlite3"),
            "released",
            &mut diagnosis,
        );
        let deferred = finding(
            &diagnosis,
            FINDING_RELEASED_TRANSCRIPT_GRAPH_SEMANTIC_VERIFICATION_DEFERRED,
        )
        .expect("released graph must report deferred semantic verification");
        assert_eq!(deferred.severity, FindingSeverity::Warning);
        assert!(
            deferred
                .message
                .contains("semantic_verification_deferred=true"),
            "{}",
            deferred.message
        );
    }

    #[test]
    fn current_inline_graph_corruption_is_not_hidden_by_raw_blob_sweep() {
        let document = format!(
            r#"{{
                "messages": [],
                "metadata": {{
                    "session_transcript_history_state_v1": {{
                        "format": "{TRANSCRIPT_HISTORY_FORMAT_CURRENT}",
                        "anchor": {{}},
                        "edges": []
                    }}
                }}
            }}"#
        );
        assert!(
            collect_inline_session_blob_refs(document.as_bytes()).is_err(),
            "raw live-message inspection must retain core-owned current graph validation"
        );
    }

    #[tokio::test]
    async fn unmeasurable_rows_report_unknown_instead_of_healthy() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "unknown", "sqlite");
        let db_path = realm_dir.join("sessions.sqlite3");
        let head_less_id = Session::new().id().clone();
        let null_head_id = Session::new().id().clone();
        let message = strand_message(512 * 1024);
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(SESSIONS_DDL).unwrap();
            // Deliberately permissive DDL: the real schema's NOT NULL is
            // exactly what doctor may not assume about a corrupted or
            // foreign-written file, and a NULL durable column is the row it
            // must never silently count as zero bytes.
            conn.execute_batch(
                "CREATE TABLE session_heads (
                    session_id TEXT PRIMARY KEY, version INTEGER, strand TEXT,
                    head_revision TEXT, message_count INTEGER, rewrite_count INTEGER,
                    total_tokens INTEGER, created_at_ms INTEGER, updated_at_ms INTEGER,
                    metadata_json TEXT, head_json BLOB, cas_token TEXT)",
            )
            .unwrap();
            conn.execute_batch(SESSION_STRAND_MESSAGES_DDL).unwrap();
            conn.execute(
                "INSERT INTO session_heads (session_id, version, strand, head_revision, \
                 message_count, rewrite_count, total_tokens, created_at_ms, updated_at_ms, \
                 metadata_json, head_json, cas_token) \
                 VALUES (?1, 1, 'root', 'digest', 1, 0, 0, 0, 0, '{}', NULL, 'cas')",
                rusqlite::params![null_head_id.to_string()],
            )
            .unwrap();
            insert_strand_row(&conn, &null_head_id, "root", 0, &message);
            for revision in 0..3u64 {
                let strand = format!("rewrite:{revision}");
                insert_strand_row(&conn, &null_head_id, &strand, 0, &message);
            }
            // A blob document that is not JSON at all.
            conn.execute(
                "INSERT INTO sessions (session_id, created_at_ms, updated_at_ms, message_count, \
                 total_tokens, metadata_json, session_json) VALUES (?1, 0, 0, 1, 0, '{}', ?2)",
                rusqlite::params![head_less_id.to_string(), b"not-json".as_slice()],
            )
            .unwrap();
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;

        let unmeasured = finding(&diagnosis, FINDING_STORAGE_CENSUS_UNMEASURED)
            .expect("unmeasured census finding");
        assert_eq!(unmeasured.severity, FindingSeverity::Warning);
        assert_eq!(unmeasured.realm.as_deref(), Some("unknown"));
        assert!(
            unmeasured
                .message
                .contains("could not measure 1 durable row(s) and 1 session document(s)"),
            "{}",
            unmeasured.message
        );
        assert!(
            unmeasured.message.contains("UNKNOWN"),
            "{}",
            unmeasured.message
        );
        // 1.5 MiB of retained strand rows sit behind that head row, and the
        // census still refuses to publish a ratio it cannot total.
        assert!(
            !codes(&diagnosis).contains(&FINDING_TRANSCRIPT_HISTORY_OVERSIZED),
            "{diagnosis:?}"
        );
        // Every aggregate says out loud that it is partial.
        let pool = finding(&diagnosis, FINDING_STRAND_DUPLICATION_RECLAIMABLE)
            .expect("strand pool finding");
        assert!(pool.message.contains("partial census"), "{}", pool.message);
        // The undecodable document is still reported by the blob sweep.
        assert!(codes(&diagnosis).contains(&FINDING_SESSION_DOCUMENT_UNDECODABLE));
    }

    #[tokio::test]
    async fn headless_strand_rows_are_not_misread_as_inline_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "midmint", "sqlite");
        let db_path = realm_dir.join("sessions.sqlite3");
        // `append_messages` lands strand rows in one transaction and
        // `save_head` mints the head row in a later one
        // (meerkat-store/src/sqlite_store.rs), so a session legitimately
        // holds strand rows and no head row — and no `sessions.session_json`
        // row at all. Enough mass that the old mass-alone arm would have
        // fired.
        let headless_id = Session::new().id().clone();
        let headless_message = strand_message(512 * 1024);
        // A genuinely oversized head-canonical neighbour: the head-less
        // session must not dilute or join its ratio findings.
        let oversized_id = Session::new().id().clone();
        let oversized_message = strand_message(64 * 1024);
        {
            let conn = head_canonical_db(&db_path);
            for seq in 0..4 {
                insert_strand_row(&conn, &headless_id, "root", seq, &headless_message);
            }
            insert_head(&conn, &oversized_id, "root", 4);
            for seq in 0..4 {
                insert_strand_row(&conn, &oversized_id, "root", seq, &oversized_message);
            }
            for revision in 0..6u64 {
                for seq in 0..4 {
                    insert_strand_row(
                        &conn,
                        &oversized_id,
                        &format!("rewrite:{revision}"),
                        seq,
                        &oversized_message,
                    );
                }
            }
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;

        // The head-less session's bytes live in `session_strand_messages`;
        // no finding may place them — or anything else — in an inline
        // document it does not have.
        assert!(
            !diagnosis
                .findings
                .iter()
                .any(|f| f.message.contains("inline in sessions.session_json")),
            "{diagnosis:?}"
        );
        assert!(
            !diagnosis.findings.iter().any(|f| {
                f.code == FINDING_TRANSCRIPT_HISTORY_OVERSIZED
                    && f.message.contains(&headless_id.to_string())
            }),
            "an unknowable live-or-retained split must not be reported as a \
             measured ratio: {diagnosis:?}"
        );

        // The genuinely oversized neighbour still warns, in its real
        // location, and the summary counts the head-less session as
        // excluded rather than measured-and-healthy.
        let per_session = diagnosis
            .findings
            .iter()
            .find(|f| {
                f.code == FINDING_TRANSCRIPT_HISTORY_OVERSIZED
                    && f.message.contains(&oversized_id.to_string())
            })
            .expect("head-canonical footprint finding");
        assert!(
            per_session
                .message
                .contains("session_strand_messages + session_rewrites"),
            "{}",
            per_session.message
        );
        let summary = diagnosis
            .findings
            .iter()
            .find(|f| {
                f.code == FINDING_TRANSCRIPT_HISTORY_OVERSIZED
                    && f.message.contains("measured session(s) exceed")
            })
            .expect("database-wide summary finding");
        assert!(
            summary.message.contains("1 of 1 measured session(s)"),
            "{}",
            summary.message
        );
        assert!(
            summary.message.contains("1 head-less strand session(s)"),
            "{}",
            summary.message
        );

        // Not silently healthy either: the strand pool carries the rows as
        // unclassified, and the census says the footprint is unknown.
        let pool = finding(&diagnosis, FINDING_STRAND_DUPLICATION_RECLAIMABLE)
            .expect("strand pool finding");
        let unclassified_clause = format!(
            "4 row(s) / {} belong to sessions with no usable `session_heads` row",
            format_bytes(4 * headless_message.len() as u64)
        );
        assert!(
            pool.message.contains(&unclassified_clause),
            "{}",
            pool.message
        );
        let unmeasured =
            finding(&diagnosis, FINDING_STORAGE_CENSUS_UNMEASURED).expect("census gap finding");
        assert!(
            unmeasured
                .message
                .contains("could not classify 1 session(s) holding strand rows"),
            "{}",
            unmeasured.message
        );
        assert!(
            unmeasured.message.contains("UNKNOWN"),
            "{}",
            unmeasured.message
        );
        assert!(!diagnosis.has_errors(), "{diagnosis:?}");
    }

    #[tokio::test]
    async fn spliced_away_strands_still_count_as_retained_revisions() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "spliced", "sqlite");
        let db_path = realm_dir.join("sessions.sqlite3");
        let id = Session::new().id().clone();
        let live = strand_message(1024);
        let retained = strand_message(512 * 1024);
        {
            let conn = head_canonical_db(&db_path);
            insert_head(&conn, &id, "root", 1);
            insert_strand_row(&conn, &id, "root", 0, &live);
            for seq in 0..3 {
                insert_strand_row(&conn, &id, "rewrite:0", seq, &retained);
            }
            // A revision spliced away entirely: it survives only as a
            // supersession link, with no row of its own.
            conn.execute(
                "INSERT INTO session_strand_links (session_id, strand, successor, strand_len, \
                 splice_start, splice_end, successor_end, created_at_ms) \
                 VALUES (?1, 'rewrite:1', 'root', 1, 0, 0, 1, 0)",
                rusqlite::params![id.to_string()],
            )
            .unwrap();
        }

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;

        let per_session = diagnosis
            .findings
            .iter()
            .find(|f| f.code == FINDING_TRANSCRIPT_HISTORY_OVERSIZED)
            .expect("per-session footprint finding");
        assert!(
            per_session
                .message
                .contains("2 retained revision strand(s) and 0 rewrite commit(s)"),
            "the link-only revision must be counted: {}",
            per_session.message
        );
        let pool = finding(&diagnosis, FINDING_STRAND_DUPLICATION_RECLAIMABLE)
            .expect("strand pool finding");
        assert!(
            pool.message
                .contains("1 `session_strand_links` supersession row(s)"),
            "{}",
            pool.message
        );
        assert!(!diagnosis.has_errors(), "{diagnosis:?}");
    }

    #[tokio::test]
    async fn empty_and_absent_session_stores_produce_no_census_findings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let empty_dir = write_manifest(&root, "empty", "sqlite");
        drop(head_canonical_db(&empty_dir.join("sessions.sqlite3")));
        // A realm with no session database at all.
        write_manifest(&root, "absent", "sqlite");

        let diagnosis = diagnose_disk_roots(&scope(&[&root])).await;

        for code in [
            FINDING_TRANSCRIPT_HISTORY_OVERSIZED,
            FINDING_STRAND_DUPLICATION_RECLAIMABLE,
            FINDING_FROZEN_BLOB_ARCHIVE_RECLAIMABLE,
            FINDING_STORAGE_CENSUS_UNMEASURED,
        ] {
            assert!(!codes(&diagnosis).contains(&code), "{code}: {diagnosis:?}");
        }
        assert!(!diagnosis.has_errors(), "{diagnosis:?}");
        assert_eq!(diagnosis.inventory.len(), 2);
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

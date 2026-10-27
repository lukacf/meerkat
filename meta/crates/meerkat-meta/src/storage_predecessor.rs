//! One-shot adoption of predecessor (pre-0.9) state.
//!
//! Before 0.9 this product shipped under a different name, and several
//! durable names carried it: two database file names, the gateway's memory
//! ledger file, five schema ledger domains, one SQLite table, the gateway
//! home directory, the flow-editor draft store file, the steward dream
//! schedule's host runnable, the creator field of the composition manifest
//! beside each persistent mob storage and the public marker in a host's
//! document MCP descriptors. 0.9 renames all of them.
//! This module is the only place that knows the predecessor spellings, and
//! it uses them for exactly three things:
//!
//! 1. **Typed refusal.** [`predecessor_state_files`] and
//!    [`predecessor_gateway_home_beside`] let the layout refuse to open a
//!    state directory or derive a gateway home that still holds predecessor
//!    names ([`crate::storage_layout::StorageLayoutError::PredecessorState`],
//!    [`crate::storage_layout::StorageLayoutError::PredecessorGatewayHome`]).
//!    Nothing ever reads a predecessor name as a fallback: a renamed default
//!    never starts empty beside existing state. The extension document
//!    store kept its file name, so its predecessor ledger domain is what
//!    marks it.
//! 2. **Explicit adoption.** [`adopt_predecessor_state`], behind
//!    `meta_gateway storage-adopt-predecessor`, carries one stopped
//!    installation (a state directory and optionally its gateway home) over,
//!    offline. Dry-run by default, and read-only: planning opens databases
//!    and the schedule store with the read-only profile and never through
//!    their owners. Apply fences every database it touches, including the
//!    renamed destinations, revalidates the plan under the fence, writes the
//!    adoption journal, and then runs its steps in order: database file
//!    renames (WAL-safe, with registered rename markers), the memory ledger
//!    file (a same-directory rename of its committed bytes), ledger domain
//!    rows and the metadata table, the composition manifests (their creator
//!    field renamed in place, every other byte kept), the steward dream
//!    schedule (through the schedule service), the gateway home with its
//!    established peer key, host pins and control audiences, and the draft
//!    store. The run stops at the first failure. Every refusal happens
//!    before the first change; every failure after it is reported as such,
//!    never as "unchanged". Given the mob definitions the installation boots
//!    with ([`PredecessorDefinitionCheck`]), planning also names each key
//!    that the predecessor release which created a store ignored, that Meta
//!    0.9 applies and that the definition the store holds does not record,
//!    and refuses until the operator removes or acknowledges it. To read
//!    that definition it copies the store's event log from a read-only
//!    connection into a private temporary directory and opens only the copy
//!    through Meerkat's mob storage; adoption itself never changes a
//!    definition.
//! 3. **Two read aliases.** The composition manifest reads its creator
//!    field under [`PREDECESSOR_MANIFEST_CREATOR_FIELD`] too, and a
//!    document MCP descriptor resolves with its marker spelled
//!    [`PREDECESSOR_DOCUMENT_BINDING_ENV`] too. Each name is a field inside
//!    data the store keeps, not a file or table name a renamed default could
//!    start empty beside, so reading it cannot fork state. The manifest
//!    alias keeps a store that was adopted before the manifest step existed
//!    opening; [`repair_composition_manifest`] (`meta_repair
//!    --composition-manifests`) renames it on such a store. The marker
//!    alias keeps a descriptor that a mob definition persisted before 0.9
//!    resolving to its host binding: definitions are event-sourced, so no
//!    adoption step rewrites them. Unresolved, a descriptor in either
//!    spelling refuses instead of launching.
//!
//! The journal ([`ADOPTION_JOURNAL_FILE_NAME`]) is the completion evidence.
//! Until it records every step finished, startup refuses
//! ([`crate::storage_layout::StorageLayoutError::AdoptionIncomplete`]) and a
//! rerun resumes each step from where the files actually lie.
//!
//! The maintenance fence covers SQLite databases only. Nothing fences the
//! memory ledger's JSON writer: the stopped installation is what keeps it
//! away, and revalidating the ledger's bytes under the fence detects a
//! change without excluding one. The composition manifests are JSON files
//! too, written only when their mob storage is created; the stock event log
//! beside its manifest is fenced, and each manifest's bytes are revalidated
//! the same way before its creator field is renamed. Directory fsyncs, which
//! make each rename durable, run on Unix only.
//!
//! Session rows the predecessor repair tool rewrote keep their markers;
//! the repair tool recognizes them ([`PREDECESSOR_REPAIR_MARKER_PREFIX`])
//! so it never bounds a row twice.
//!
//! The meerkat-level realm id is per installation
//! ([`resolve_installation`], resolved once when a layout is constructed): a
//! fresh installation uses the Meta default, and an adopted one keeps the
//! predecessor realm id, recorded in its journal and validated. Its realm
//! manifest, detached-job rows and external backends are keyed by it, so
//! nothing under it is rewritten.
//!
//! Three values are frozen durable identifiers rather than names, and keep
//! their predecessor values in every installation: the console
//! storage-scope context, the delivery-correlation namespace and the
//! agent-memory tool source id. Each keys existing data (saved browser
//! queues, delivery dedupe ids, persisted tool visibility witnesses); none
//! is a signing or verification context.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use meerkat::{
    HostRunnableName, HostRunnableTargetBinding, ScheduleService, SqliteScheduleStore,
    TargetBinding, UpdateScheduleRequest,
};
use meerkat_mob::MobDefinition;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::mob_composition_manifest::MobCompositionManifest;
use crate::storage_layout::{
    CANONICAL_CONSOLE_DB_FILE_NAME, CANONICAL_METADATA_DB_FILE_NAME, GATEWAY_HOME_DIR_NAME,
    MetaStorageLayout,
};
use crate::storage_migrate::{
    MetaMaintenanceFence, MigrateMode, enumerate_state_dir_databases, move_database_file,
    write_rename_marker,
};

/// Predecessor spellings of the runtime-metadata database, canonical first.
pub const PREDECESSOR_METADATA_DB_FILE_NAMES: [&str; 2] =
    ["mobkit_metadata.sqlite3", "mobkit_metadata.sqlite"];
/// Predecessor spellings of the console-timeline database, canonical first.
pub const PREDECESSOR_CONSOLE_DB_FILE_NAMES: [&str; 2] =
    ["mobkit_console.sqlite3", "mobkit_console.sqlite"];
/// Predecessor schema ledger domains and the domains they become. The
/// extension document store's domain ([`PREDECESSOR_EXTENSION_LEDGER_DOMAIN`])
/// is renamed beside them.
pub const PREDECESSOR_LEDGER_DOMAINS: [(&str, &str); 4] = [
    ("mobkit-continuity", "meta-continuity"),
    ("mobkit-metadata", "meta-metadata"),
    ("mobkit-console", "meta-console"),
    ("mobkit-memory", "meta-memory"),
];
/// Predecessor ledger domain of the extension document store
/// ([`crate::storage_layout::EXTENSION_STATE_DB_FILE_NAME`]). The store kept
/// its file name and its schema; only this domain marks it as predecessor
/// state. Adoption renames it to `meta-extension-state`.
pub const PREDECESSOR_EXTENSION_LEDGER_DOMAIN: &str = "mobkit-extension-state";
/// The extension document store's ledger domain since 0.9.
const EXTENSION_LEDGER_DOMAIN: &str = "meta-extension-state";
/// Predecessor spelling of the public marker in a host's document MCP
/// descriptors (`META_MCP_DOCUMENT_BINDING`). A descriptor a profile
/// persisted before 0.9 carries it; the document binding resolves it as
/// the current spelling.
pub const PREDECESSOR_DOCUMENT_BINDING_ENV: &str = "MOBKIT_MCP_DOCUMENT_BINDING";
/// The predecessor's meerkat-level realm id. An adopted installation keeps
/// it, recorded in its adoption journal; a fresh installation uses
/// [`crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID`].
pub const PREDECESSOR_REALM_ID: &str = "mobkit";
/// Predecessor name of the runtime-metadata table.
pub const PREDECESSOR_METADATA_TABLE: &str = "mobkit_metadata";
/// Predecessor name of the gateway home directory.
pub const PREDECESSOR_GATEWAY_HOME_DIR_NAME: &str = "meerkat-mobkit";
/// Predecessor name of the flow-editor draft store file in the gateway home.
pub const PREDECESSOR_DRAFT_STORE_FILE_NAME: &str = "meerkat-mobkit-flow-editor-drafts.json";
/// Predecessor name of the stock gateway's memory ledger file in its state
/// directory; adoption renames it to
/// [`crate::storage_layout::MEMORY_LEDGER_FILE_NAME`]. Only this one fixed
/// path is adopted: a ledger an embedder configured at another `state_path`
/// is the embedder's to rename.
pub const PREDECESSOR_MEMORY_LEDGER_FILE_NAME: &str = "elephant-memory-state.json";
/// Predecessor name of the steward dream host runnable.
pub const PREDECESSOR_STEWARD_DREAM_RUNNABLE: &str = "mobkit.memory.steward.dream";
/// Predecessor spelling of the creator field in the composition manifest
/// beside each persistent mob storage
/// ([`crate::mob_composition_manifest::MobCompositionManifest::created_by_meta`]).
/// The manifest reader accepts it as an alias; adoption (its
/// [`AdoptionStep::Ledger`] step) and `meta_repair --composition-manifests`
/// ([`repair_composition_manifest`]) rename it on disk.
pub const PREDECESSOR_MANIFEST_CREATOR_FIELD: &str = "created_by_mobkit";
/// The creator field's current spelling.
const MANIFEST_CREATOR_FIELD: &str = "created_by_meta";
/// The environment variables the predecessor (pre-0.9) gateways, repair
/// tool, flow editor and SDKs read. Each is now spelled `META_` with the
/// same suffix. Matched by exact name: a host's own variables that merely
/// contain the predecessor name (`HOST_MOBKIT_STATE_DIR`) or start with it
/// without being one the predecessor read (`MOBKIT_MCP_FIXTURE_BIN`) are
/// not predecessor configuration and are left alone.
pub const PREDECESSOR_ENV_VARS: &[&str] = &[
    "MOBKIT_ACTIVATION_STALL_RETRIES",
    "MOBKIT_ACTOR_LOOP_PROBE_BUDGET_SECS",
    "MOBKIT_ACTOR_LOOP_PROBE_INTERVAL_SECS",
    "MOBKIT_AGENT_MEMORY_SELECTOR",
    "MOBKIT_BRIDGE_ACTOR_ADMISSION_SECS",
    "MOBKIT_CONSOLE_PROGRESS_MEMBER_CAP",
    "MOBKIT_CONSOLE_READ_ONLY",
    "MOBKIT_CONSOLE_READ_TIMEOUT_SECS",
    "MOBKIT_CONTRACT_VERSION",
    "MOBKIT_EXAMPLE_BIN_DIR",
    "MOBKIT_FLOW_EDITOR_ALLOW_HOST_DEPLOY",
    "MOBKIT_FLOW_EDITOR_BIN",
    "MOBKIT_FLOW_EDITOR_DRAFT_STORE",
    "MOBKIT_FLOW_EDITOR_LISTEN_ADDR",
    "MOBKIT_FLOW_EDITOR_RPC_URL",
    "MOBKIT_GATEWAY_BIN",
    "MOBKIT_GATEWAY_STDERR",
    "MOBKIT_GATEWAY_STDERR_FILE",
    "MOBKIT_HTTP_ALLOW_REMOTE",
    "MOBKIT_HTTP_LISTEN_ADDR",
    "MOBKIT_IDENTITY_RESTORE_CONCURRENCY",
    "MOBKIT_MCP_SOURCES",
    "MOBKIT_MEMORY_OBSERVATION",
    "MOBKIT_MODULE_BOUNDARY",
    "MOBKIT_REF_ADDR",
    "MOBKIT_REF_HTTP_MODE",
    "MOBKIT_ROOT",
    "MOBKIT_RPC_GATEWAY_BIN",
    "MOBKIT_RPC_REQUEST",
    "MOBKIT_RUST_TOOL_BUNDLES",
    "MOBKIT_SKILL_DIRS",
    "MOBKIT_TRACE_RUNTIME_TURNS",
    "MOBKIT_VOICE_GATEWAY_BIN",
];
/// Archive directory of predecessor mobpack exports (`<dir>/editor.json`,
/// `<dir>/mob.toml`).
pub const PREDECESSOR_MOBPACK_ARCHIVE_DIR: &str = "mobkit/";

/// Lead-in of the tool-result markers the predecessor repair tool wrote into
/// session rows. The repair tool treats a row carrying it as already
/// bounded, so a second pass over a predecessor-repaired transcript does not
/// shave it again and overwrite its recorded original size.
pub const PREDECESSOR_REPAIR_MARKER_PREFIX: &str = "[mobkit-repair:tool-result-";

/// How long the fence waits for in-flight operations to drain.
const FENCE_DRAIN_DEADLINE: Duration = Duration::from_secs(10);

/// Every predecessor database file present in `state_dir`, sorted.
pub fn predecessor_database_files(state_dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = PREDECESSOR_METADATA_DB_FILE_NAMES
        .iter()
        .chain(PREDECESSOR_CONSOLE_DB_FILE_NAMES.iter())
        .map(|name| state_dir.join(name))
        .filter(|path| path.is_file())
        .collect();
    found.sort();
    found
}

/// Every predecessor file present in `state_dir`, sorted: the database
/// files, the memory ledger file together with the stage an interrupted
/// write of it leaves behind, and the extension document store while its
/// ledger carries the predecessor domain. Startup refuses on any of them,
/// whether or not it enables memory or extension state, before a ledger can
/// start empty beside them or a store opens under a domain it does not
/// carry.
pub fn predecessor_state_files(state_dir: &Path) -> Vec<PathBuf> {
    let ledger = state_dir.join(PREDECESSOR_MEMORY_LEDGER_FILE_NAME);
    let mut found = predecessor_database_files(state_dir);
    found.extend(
        [crate::runtime::staged_ledger_path(&ledger), ledger]
            .into_iter()
            .filter(|path| present(path)),
    );
    found.extend(predecessor_extension_store(state_dir));
    found.sort();
    found
}

/// The extension document store, when its ledger still carries the
/// predecessor domain. Its file name is unchanged, so only its ledger tells
/// it apart. A store this read-only probe cannot read is left to its own
/// open, which refuses an unreadable file and never adopts one.
fn predecessor_extension_store(state_dir: &Path) -> Option<PathBuf> {
    let path = state_dir.join(crate::storage_layout::EXTENSION_STATE_DB_FILE_NAME);
    if !path.is_file() {
        return None;
    }
    let conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::ReadOnly).ok()?;
    read_ledger_domains(&conn)
        .ok()
        .flatten()?
        .iter()
        .any(|domain| domain == PREDECESSOR_EXTENSION_LEDGER_DOMAIN)
        .then_some(path)
}

/// Whether anything (a file, a directory, a link, even a dangling one) lies
/// at `path`.
fn present(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// The predecessor sibling of a gateway home that uses the default
/// directory name; `None` for an explicitly named home.
pub fn predecessor_gateway_home_beside(home: &Path) -> Option<PathBuf> {
    if home.file_name().and_then(|name| name.to_str()) != Some(GATEWAY_HOME_DIR_NAME) {
        return None;
    }
    home.parent()
        .map(|parent| parent.join(PREDECESSOR_GATEWAY_HOME_DIR_NAME))
}

/// The predecessor environment variables set in this process, sorted.
pub fn predecessor_environment_variables() -> Vec<String> {
    predecessor_variables_among(std::env::vars_os().filter_map(|(name, _)| name.into_string().ok()))
}

/// The predecessor variables among `names` (exact-name match), sorted.
pub fn predecessor_variables_among(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut found: Vec<String> = names
        .into_iter()
        .filter(|name| PREDECESSOR_ENV_VARS.contains(&name.as_str()))
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Refuse to start while predecessor environment variables are set: each
/// one is configuration the renamed binary would otherwise silently ignore.
pub fn refuse_predecessor_environment() -> Result<(), PredecessorEnvironmentError> {
    let names = predecessor_environment_variables();
    if names.is_empty() {
        Ok(())
    } else {
        Err(PredecessorEnvironmentError { names })
    }
}

/// Predecessor environment variables are set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredecessorEnvironmentError {
    pub names: Vec<String>,
}

impl std::fmt::Display for PredecessorEnvironmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "predecessor (pre-0.9) environment variables are set and would be ignored: {}; \
             rename each to its META_ spelling (or unset it) before starting",
            self.names.join(", ")
        )
    }
}

impl std::error::Error for PredecessorEnvironmentError {}

/// File name of the adoption journal in the state directory.
pub const ADOPTION_JOURNAL_FILE_NAME: &str = "meta-adoption.json";

/// One adoption step, in execution order.
///
/// A journal is decoded strictly. In an unfinished journal written before a
/// step existed, that step is pending and a rerun runs it. A journal that
/// records a step an older build does not know is unreadable to that build,
/// which refuses it as [`AdoptionJournalUnreadable`] rather than guessing.
///
/// [`AdoptionJournalUnreadable`]: crate::storage_layout::StorageLayoutError::AdoptionJournalUnreadable
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptionStep {
    DatabaseFiles,
    /// The stock gateway's memory ledger file: a same-directory rename of
    /// its committed bytes to the canonical name.
    MemoryLedgerFile,
    /// Every in-place rename of a predecessor name inside stored data: the
    /// schema ledger domain rows and the metadata table, then the creator
    /// field ([`PREDECESSOR_MANIFEST_CREATOR_FIELD`]) of every composition
    /// manifest in the state directory, every other manifest byte kept.
    ///
    /// The manifests ride this step rather than a step of their own so a
    /// journal stays readable by every 0.9 release: an earlier one refuses a
    /// step name it does not know (see above), which would turn adoption into
    /// a one-way door against a binary rollback.
    Ledger,
    StewardSchedule,
    GatewayHome,
    DraftStore,
}

impl AdoptionStep {
    /// Every step, in execution order.
    pub const ALL: [Self; 6] = [
        Self::DatabaseFiles,
        Self::MemoryLedgerFile,
        Self::Ledger,
        Self::StewardSchedule,
        Self::GatewayHome,
        Self::DraftStore,
    ];
}

/// Whether an adoption finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptionStatus {
    /// Adoption changed state and has not finished every step. Startup
    /// refuses until a rerun completes it.
    InProgress,
    /// Every step finished.
    Complete,
}

/// The adoption journal: the completion evidence for one installation.
///
/// Written before the first change of an apply run and updated after every
/// step, so an interrupted adoption is identified by its journal, never by
/// the absence of predecessor file names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionJournal {
    pub version: u32,
    pub status: AdoptionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_home: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_home: Option<PathBuf>,
    #[serde(default)]
    pub completed: BTreeSet<AdoptionStep>,
    /// The meerkat-level realm id this adopted installation keeps (its
    /// jobs, outbox and realm manifest are keyed by it). Absent when the
    /// predecessor installation had no realm directory: the installation
    /// then uses the fresh default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
}

const ADOPTION_JOURNAL_VERSION: u32 = 1;

/// Read the adoption journal of `state_dir`; `Ok(None)` when there is none.
/// An unreadable or undecodable journal is an error: startup treats it as
/// an unfinished adoption.
pub fn read_adoption_journal(state_dir: &Path) -> Result<Option<AdoptionJournal>, String> {
    let path = state_dir.join(ADOPTION_JOURNAL_FILE_NAME);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let journal: AdoptionJournal = serde_json::from_slice(&bytes)
        .map_err(|error| format!("decode {}: {error}", path.display()))?;
    if journal.version != ADOPTION_JOURNAL_VERSION {
        return Err(format!(
            "{} has unsupported version {}",
            path.display(),
            journal.version
        ));
    }
    Ok(Some(journal))
}

/// Replace the journal durably: write a sibling, fsync it, rename it over
/// the journal, then fsync the state directory so the rename itself
/// survives a crash.
fn write_adoption_journal(
    state_dir: &Path,
    journal: &AdoptionJournal,
) -> Result<(), JournalWriteError> {
    use std::io::Write as _;
    let path = state_dir.join(ADOPTION_JOURNAL_FILE_NAME);
    let staged = staged_journal_path(state_dir);
    let bytes = serde_json::to_vec_pretty(journal).map_err(|error| JournalWriteError::Staging {
        staged: staged.clone(),
        detail: error.to_string(),
        left_behind: false,
    })?;
    // This invocation's stage is created exclusively: a stage that already
    // exists is someone else's evidence (an interrupted run's newest
    // progress) and is never truncated or removed here.
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(JournalWriteError::StagePresent { staged });
        }
        Err(error) => {
            return Err(JournalWriteError::Staging {
                detail: format!("create {}: {error}", staged.display()),
                staged,
                left_behind: false,
            });
        }
    };
    // From here the stage is ours: a failure removes it, and only it.
    let staging_failed = |detail: String| {
        let left_behind = match std::fs::remove_file(&staged) {
            Ok(()) => false,
            Err(error) => error.kind() != std::io::ErrorKind::NotFound,
        };
        JournalWriteError::Staging {
            staged: staged.clone(),
            detail,
            left_behind,
        }
    };
    let written = file.write_all(&bytes).and_then(|()| file.sync_all());
    drop(file);
    written.map_err(|error| staging_failed(format!("write {}: {error}", staged.display())))?;
    std::fs::rename(&staged, &path).map_err(|error| {
        staging_failed(format!(
            "rename {} -> {}: {error}",
            staged.display(),
            path.display()
        ))
    })?;
    // The journal is replaced; only the directory entry's durability is
    // left to establish.
    sync_dir(state_dir).map_err(|detail| JournalWriteError::Durability { detail })
}

/// The staged sibling a journal write goes through.
fn staged_journal_path(state_dir: &Path) -> PathBuf {
    state_dir.join(format!("{ADOPTION_JOURNAL_FILE_NAME}.next"))
}

/// Record that this installation keeps the predecessor realm, established
/// by an explicit `AdoptedPredecessor` declaration over state that recorded
/// no realm. The record is the adoption journal's realm id, so the
/// installation resolver recognizes the realm on every later start (and
/// refuses a contradicting declaration); nothing else is adopted.
pub(crate) fn record_declared_predecessor_realm(state_dir: &Path) -> Result<(), String> {
    let existing = read_adoption_journal(state_dir)?;
    if existing
        .as_ref()
        .is_some_and(|journal| journal.realm_id.as_deref() == Some(PREDECESSOR_REALM_ID))
    {
        return Ok(());
    }
    let mut journal = existing.unwrap_or(AdoptionJournal {
        version: ADOPTION_JOURNAL_VERSION,
        status: AdoptionStatus::Complete,
        predecessor_home: None,
        gateway_home: None,
        completed: AdoptionStep::ALL.into(),
        realm_id: None,
    });
    journal.realm_id = Some(PREDECESSOR_REALM_ID.to_string());
    write_adoption_journal(state_dir, &journal).map_err(|error| error.to_string())
}

/// How a journal write failed, and therefore what it left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
enum JournalWriteError {
    /// A staged journal already exists. It was not touched.
    StagePresent { staged: PathBuf },
    /// The journal was not replaced. `left_behind` says whether this
    /// invocation's own stage could not be removed again.
    Staging {
        staged: PathBuf,
        detail: String,
        left_behind: bool,
    },
    /// The journal was replaced, but its directory entry could not be made
    /// durable.
    Durability { detail: String },
}

impl std::fmt::Display for JournalWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StagePresent { staged } => write!(
                f,
                "a staged journal {} from an earlier run exists and was left untouched; it may \
                 hold that run's newest progress. Inspect it: move it over the journal if it \
                 decodes and matches the installation, otherwise remove it, then rerun",
                staged.display()
            ),
            Self::Staging {
                staged,
                detail,
                left_behind: false,
            } => write!(f, "{detail} (no staged {} was left)", staged.display()),
            Self::Staging {
                staged,
                detail,
                left_behind: true,
            } => write!(
                f,
                "{detail}; the staged {} could not be removed and is left behind",
                staged.display()
            ),
            Self::Durability { detail } => {
                write!(
                    f,
                    "the journal was replaced but is not yet durable: {detail}"
                )
            }
        }
    }
}

/// fsync a directory so entries renamed into or out of it are durable. Unix
/// only: elsewhere a rename's durability rests on the filesystem's own
/// ordering, and nothing here promises more.
fn sync_dir(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::fs::File::open(dir)
            .and_then(|handle| handle.sync_all())
            .map_err(|error| format!("fsync directory {}: {error}", dir.display()))
    }
    #[cfg(not(unix))]
    {
        // Directory handles cannot be fsynced here; renames rely on the
        // filesystem's own ordering.
        let _ = dir;
        Ok(())
    }
}

/// fsync the parent directory of `path`.
fn sync_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => sync_dir(parent),
        _ => sync_dir(Path::new(".")),
    }
}

/// Resolve an installation once, when its layout is constructed: its
/// meerkat-level realm id, or the typed reason it must refuse.
///
/// - Predecessor state files ([`predecessor_state_files`]: database files,
///   the memory ledger file or its interrupted-write stage), whether or not
///   this start enables memory: [`StorageLayoutError::PredecessorState`].
/// - An unfinished adoption journal:
///   [`StorageLayoutError::AdoptionIncomplete`].
/// - An unreadable (for example torn) adoption journal:
///   [`StorageLayoutError::AdoptionJournalUnreadable`], naming the manual
///   recovery.
/// - Both realm directories present: [`StorageLayoutError::RealmTwin`].
/// - A recorded realm id: validated (it must be the predecessor realm id,
///   parse as a realm id, and name an existing realm directory), else
///   [`StorageLayoutError::AdoptedRealmInvalid`]. Never replaced by the
///   default.
/// - No recorded realm id while the predecessor realm directory exists:
///   [`StorageLayoutError::PredecessorState`] for an unadopted directory,
///   [`StorageLayoutError::AdoptedRealmInvalid`] for an adopted one.
/// - Otherwise the fresh default.
///
/// [`StorageLayoutError::PredecessorState`]: crate::storage_layout::StorageLayoutError::PredecessorState
/// [`StorageLayoutError::AdoptionIncomplete`]: crate::storage_layout::StorageLayoutError::AdoptionIncomplete
/// [`StorageLayoutError::AdoptionJournalUnreadable`]: crate::storage_layout::StorageLayoutError::AdoptionJournalUnreadable
/// [`StorageLayoutError::RealmTwin`]: crate::storage_layout::StorageLayoutError::RealmTwin
/// [`StorageLayoutError::AdoptedRealmInvalid`]: crate::storage_layout::StorageLayoutError::AdoptedRealmInvalid
pub fn resolve_installation(
    state_dir: &Path,
) -> Result<meerkat_core::RealmId, crate::storage_layout::StorageLayoutError> {
    use crate::storage_layout::StorageLayoutError;
    let predecessor = predecessor_state_files(state_dir);
    if !predecessor.is_empty() {
        return Err(StorageLayoutError::PredecessorState {
            state_dir: state_dir.to_path_buf(),
            paths: predecessor,
        });
    }
    let journal_path = state_dir.join(ADOPTION_JOURNAL_FILE_NAME);
    let journal = read_adoption_journal(state_dir).map_err(|detail| {
        StorageLayoutError::AdoptionJournalUnreadable {
            path: journal_path.clone(),
            detail,
        }
    })?;
    if let Some(detail) = journal.as_ref().and_then(unfinished_steps) {
        return Err(StorageLayoutError::AdoptionIncomplete {
            path: journal_path,
            detail,
        });
    }
    let predecessor_realm = state_dir.join(PREDECESSOR_REALM_ID);
    let default_realm = state_dir.join(crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID);
    if predecessor_realm.exists() && default_realm.exists() {
        return Err(StorageLayoutError::RealmTwin {
            paths: vec![predecessor_realm, default_realm],
        });
    }
    let adopted = journal.is_some();
    match journal.and_then(|journal| journal.realm_id) {
        Some(recorded) => {
            let invalid = |reason: &str| StorageLayoutError::AdoptedRealmInvalid {
                journal: journal_path.clone(),
                value: Some(recorded.clone()),
                reason: reason.to_string(),
            };
            if recorded != PREDECESSOR_REALM_ID {
                return Err(invalid(
                    "adoption only ever records the predecessor realm id",
                ));
            }
            let realm = meerkat_core::RealmId::parse(recorded.as_str())
                .map_err(|error| invalid(&format!("not a realm id: {error}")))?;
            if !state_dir.join(realm.as_str()).is_dir() {
                return Err(invalid("its realm directory is missing"));
            }
            Ok(realm)
        }
        None if predecessor_realm.exists() => Err(if adopted {
            StorageLayoutError::AdoptedRealmInvalid {
                journal: journal_path,
                value: None,
                reason: "the predecessor realm directory exists but the adoption journal \
                         records no realm id"
                    .into(),
            }
        } else {
            StorageLayoutError::PredecessorState {
                state_dir: state_dir.to_path_buf(),
                paths: vec![predecessor_realm],
            }
        }),
        None => Ok(crate::storage_provider::default_meerkat_level_realm()),
    }
}

/// Every detached-job database under a realm directory an installation can
/// carry (the fresh default and an adopted predecessor realm). A census for
/// fencing and diagnosis only; runtime paths come from the installation's
/// resolved realm.
pub fn realm_jobs_databases(state_dir: &Path) -> Vec<PathBuf> {
    [
        crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID,
        PREDECESSOR_REALM_ID,
    ]
    .into_iter()
    .map(|realm| meerkat_store::realm_paths_in(state_dir, realm).jobs_sqlite_path)
    .filter(|path| path.is_file())
    .collect()
}

fn unfinished_steps(journal: &AdoptionJournal) -> Option<String> {
    if journal.status == AdoptionStatus::Complete {
        return None;
    }
    let pending: Vec<String> = AdoptionStep::ALL
        .iter()
        .filter(|step| !journal.completed.contains(step))
        .map(|step| format!("{step:?}"))
        .collect();
    Some(format!("steps not finished: {}", pending.join(", ")))
}

/// Why startup must refuse `state_dir` as an unfinished adoption, if it
/// must.
pub fn unfinished_adoption(state_dir: &Path) -> Option<String> {
    match read_adoption_journal(state_dir) {
        Ok(None) => None,
        Ok(Some(journal)) => unfinished_steps(&journal),
        Err(error) => Some(error),
    }
}

/// What one adoption run covers: one installation, meaning one state
/// directory and optionally its gateway home.
#[derive(Debug, Clone, Default)]
pub struct PredecessorAdoptRequest {
    /// State directory to adopt (database files, ledgers, schedule). The
    /// adoption journal lives here.
    pub state_dir: PathBuf,
    /// Predecessor gateway home to move.
    pub predecessor_home: Option<PathBuf>,
    /// The gateway home it becomes.
    pub gateway_home: Option<PathBuf>,
    /// True when the caller named the homes; false when they are derived
    /// defaults. An unfinished adoption always resumes with the homes its
    /// journal recorded: explicit homes that differ are refused, defaults
    /// are ignored.
    pub homes_explicit: bool,
    /// Dry-run or apply.
    pub mode: MigrateMode,
}

/// The gateway homes one adoption covers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct HomeScope {
    predecessor_home: Option<PathBuf>,
    gateway_home: Option<PathBuf>,
}

impl HomeScope {
    fn describe(&self) -> String {
        let show = |path: &Option<PathBuf>| {
            path.as_ref()
                .map_or_else(|| "none".to_string(), |path| path.display().to_string())
        };
        format!(
            "predecessor home {}, gateway home {}",
            show(&self.predecessor_home),
            show(&self.gateway_home)
        )
    }
}

/// The kind of durable value one action carries over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredecessorItem {
    Custody,
    Journal,
    RealmId,
    DatabaseFile,
    MemoryLedger,
    LedgerDomain,
    MetadataTable,
    CompositionManifest,
    StewardSchedule,
    GatewayHome,
    PeerIdentity,
    DraftStore,
}

/// What happened to one action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome", content = "reason")]
pub enum PredecessorOutcome {
    /// Dry-run: an apply run would perform it.
    Planned,
    /// Performed.
    Applied,
    /// Refused while planning, before this run changed anything.
    Refused(String),
    /// Failed after this run had changed state. The adoption is
    /// incomplete: the journal records which steps finished, startup
    /// refuses, and a rerun resumes from the actual post-change locations
    /// once the cause is fixed. This item itself may be partly applied.
    Failed(String),
}

/// One adoption action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredecessorAction {
    pub item: PredecessorItem,
    /// The file, database or schedule the action touches.
    pub subject: String,
    pub from: String,
    pub to: String,
    #[serde(flatten)]
    pub outcome: PredecessorOutcome,
}

/// The full adoption report.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PredecessorAdoptReport {
    #[serde(default)]
    pub mode: MigrateMode,
    #[serde(default)]
    pub state_dir: PathBuf,
    /// True once this run changed anything (the journal is the first
    /// change). False means every refusal left the installation untouched.
    #[serde(default)]
    pub changed_state: bool,
    /// True when the journal records every step finished.
    #[serde(default)]
    pub complete: bool,
    #[serde(default)]
    pub actions: Vec<PredecessorAction>,
    /// Per persistent mob storage, the keys of a supplied mob definition
    /// that the predecessor release which created the store ignored, that
    /// Meta 0.9 applies and that the definition the store holds does not
    /// record ([`PredecessorDefinitionCheck`]).
    #[serde(default)]
    pub ignored_keys: Vec<PredecessorIgnoredKeys>,
    #[serde(default)]
    pub notes: Vec<String>,
    /// Refusals and failures. Non-empty means a nonzero exit.
    #[serde(default)]
    pub errors: Vec<String>,
}

impl PredecessorAdoptReport {
    /// True when the run must exit nonzero.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    fn record(
        &mut self,
        item: PredecessorItem,
        subject: impl Into<String>,
        from: impl Into<String>,
        to: impl Into<String>,
        outcome: PredecessorOutcome,
    ) {
        let subject = subject.into();
        match &outcome {
            PredecessorOutcome::Refused(reason) => {
                self.errors
                    .push(format!("refused, nothing changed: {subject}: {reason}"));
            }
            PredecessorOutcome::Failed(reason) => self.errors.push(format!(
                "failed after changes, adoption incomplete (inspect, then rerun to resume): \
                 {subject}: {reason}"
            )),
            PredecessorOutcome::Planned | PredecessorOutcome::Applied => {}
        }
        self.actions.push(PredecessorAction {
            item,
            subject,
            from: from.into(),
            to: to.into(),
            outcome,
        });
    }

    fn refuse(
        &mut self,
        item: PredecessorItem,
        subject: impl Into<String>,
        reason: impl Into<String>,
    ) {
        self.record(
            item,
            subject,
            "",
            "",
            PredecessorOutcome::Refused(reason.into()),
        );
    }
}

/// The mob definitions an installation boots with, given to adoption to
/// check against the persistent mob storages the predecessor created.
///
/// A predecessor release read its `mob.toml` with Meerkat's definition
/// parser, which ignored a key that release's definition did not carry: a
/// config written with the key ran without it, and a store that release
/// created recorded its definition without it. Meta 0.9 reads the key. While
/// the definition a store holds still lacks it, an authoritative resume that
/// keeps it refuses as divergent, and applying it changes what the store's
/// members do. Adoption names each such key before it changes anything, and
/// refuses until the operator removes it (keeping the behaviour the store
/// has run with) or acknowledges it ([`Self::acknowledge_ignored_keys`]). An
/// acknowledgement changes no definition: the key takes effect only once the
/// stored definition is advanced through the declared definition-update
/// ceremony, and resuming never applies it. A key the store's current
/// definition records is in force, whichever release advanced it there, and
/// is not named.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct PredecessorDefinitionCheck {
    /// Each definition with the source a report names it by, such as its
    /// `mob.toml` path.
    pub definitions: Vec<(String, MobDefinition)>,
    /// Report the keys a predecessor release ignored as acknowledged instead
    /// of refusing.
    pub acknowledge_ignored_keys: bool,
}

impl PredecessorDefinitionCheck {
    pub fn new(definitions: Vec<(String, MobDefinition)>, acknowledge_ignored_keys: bool) -> Self {
        Self {
            definitions,
            acknowledge_ignored_keys,
        }
    }
}

/// The keys a supplied mob definition sets that the predecessor release
/// which created one persistent mob storage ignored, that Meta 0.9 applies,
/// and that the definition the store holds now does not record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PredecessorIgnoredKeys {
    /// The store's composition manifest.
    pub manifest: String,
    /// The mob the store and the definition name.
    pub mob_id: String,
    /// The source of the supplied definition.
    pub source: String,
    /// The release that created the store, as its manifest records it.
    pub created_by: String,
    /// Dotted paths of the keys, as a divergence refusal names them.
    pub keys: Vec<String>,
    /// The operator acknowledged them; otherwise adoption refused.
    pub acknowledged: bool,
}

/// The first predecessor release whose mob definition carried a profile's
/// or `[runtime]`'s `model_fallback` (with its Meerkat 0.8.37); earlier
/// releases ignored the key.
const MODEL_FALLBACK_RECORDED_SINCE: (u64, u64, u64) = (0, 8, 34);
/// The first predecessor release whose mob definition carried a profile's
/// `tools.deny` (with its Meerkat 0.8.51); earlier releases ignored the key.
const TOOLS_DENY_RECORDED_SINCE: (u64, u64, u64) = (0, 8, 46);

/// `major.minor.patch` of a release version, as a composition manifest
/// records its creator; `None` for anything else.
fn release_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    let release = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(release)
}

/// The keys predecessor release `release` ignored in a mob definition, as a
/// report names them; `None` when it ignored none of them.
fn ignored_keys_of(release: (u64, u64, u64)) -> Option<&'static str> {
    if release < MODEL_FALLBACK_RECORDED_SINCE {
        Some("model_fallback and tools.deny")
    } else if release < TOOLS_DENY_RECORDED_SINCE {
        Some("tools.deny")
    } else {
        None
    }
}

/// The keys `profile` sets among those `release` ignored, as dotted paths
/// under `profiles.<name>`.
fn profile_keys_ignored_by(
    release: (u64, u64, u64),
    name: &meerkat_mob::ProfileName,
    profile: &meerkat_mob::Profile,
) -> Vec<String> {
    let mut keys = Vec::new();
    if release < MODEL_FALLBACK_RECORDED_SINCE && profile.model_fallback.is_some() {
        keys.push(format!("profiles.{name}.model_fallback"));
    }
    if release < TOOLS_DENY_RECORDED_SINCE && !profile.tools.deny.is_empty() {
        keys.push(format!("profiles.{name}.tools.deny"));
    }
    keys
}

/// The keys `supplied` sets that `held`, the definition a store holds now,
/// does not record, among the keys `created_by`, the predecessor release
/// that created the store, ignored: dotted paths, as a divergence refusal
/// names them, sorted. `None` when `created_by` is not a release version, so
/// which keys it ignored cannot be told.
///
/// A key `held` records is in force, whichever release advanced the
/// definition to record it, and is not reported. Only a profile both
/// definitions declare inline is compared; a profile `held` lacks is
/// reported by [`profiles_not_held`].
fn predecessor_ignored_keys(
    created_by: &str,
    held: &MobDefinition,
    supplied: &MobDefinition,
) -> Option<Vec<String>> {
    let release = release_version(created_by)?;
    let mut keys = Vec::new();
    if release < MODEL_FALLBACK_RECORDED_SINCE
        && supplied.runtime.model_fallback.is_some()
        && held.runtime.model_fallback.is_none()
    {
        keys.push("runtime.model_fallback".to_string());
    }
    for (name, binding) in &supplied.profiles {
        let (Some(profile), Some(stored)) = (
            binding.as_inline(),
            held.profiles
                .get(name)
                .and_then(|stored| stored.as_inline()),
        ) else {
            continue;
        };
        if release < MODEL_FALLBACK_RECORDED_SINCE
            && profile.model_fallback.is_some()
            && stored.model_fallback.is_none()
        {
            keys.push(format!("profiles.{name}.model_fallback"));
        }
        if release < TOOLS_DENY_RECORDED_SINCE
            && !profile.tools.deny.is_empty()
            && stored.tools.deny.is_empty()
        {
            keys.push(format!("profiles.{name}.tools.deny"));
        }
    }
    keys.sort();
    Some(keys)
}

/// The inline profiles `supplied` declares that `held` does not, each with
/// the keys it sets among those `created_by` ignored; profiles that set none
/// are left out. Adding such a profile is a composition change of its own:
/// an authoritative resume refuses it as divergent until the stored
/// definition is advanced, and its keys take effect with it.
fn profiles_not_held(
    created_by: &str,
    held: &MobDefinition,
    supplied: &MobDefinition,
) -> Vec<(String, Vec<String>)> {
    let Some(release) = release_version(created_by) else {
        return Vec::new();
    };
    supplied
        .profiles
        .iter()
        .filter(|(name, _)| !held.profiles.contains_key(*name))
        .filter_map(|(name, binding)| {
            let keys = profile_keys_ignored_by(release, name, binding.as_inline()?);
            (!keys.is_empty()).then(|| (name.to_string(), keys))
        })
        .collect()
}

/// The storage path a composition manifest sits beside.
fn storage_beside(manifest: &Path) -> Option<PathBuf> {
    let name = manifest.file_name()?.to_str()?;
    let storage = name.strip_suffix(crate::mob_composition_manifest::MANIFEST_FILE_SUFFIX)?;
    Some(manifest.with_file_name(storage))
}

/// The definition a persistent mob storage holds now, as Meerkat's own
/// definition authority selects it from the event log (the creation
/// definition, or the latest one a definition update advanced it to);
/// `None` when no storage lies there or it holds no mob.
///
/// Planning never opens a store through its owner, which migrates the
/// store's schema as it opens. The storage is copied from a read-only
/// connection with `VACUUM INTO`, a consistent copy that includes committed
/// WAL content and leaves the original untouched, into a private temporary
/// directory, and only that copy is opened through Meerkat's mob storage.
async fn held_definition(storage: &Path) -> Result<Option<MobDefinition>, String> {
    if !present(storage) {
        return Ok(None);
    }
    let copy_dir = tempfile::Builder::new()
        .prefix("meta-adoption-definition-")
        .tempdir()
        .map_err(|error| format!("create a private directory for its copy: {error}"))?;
    let copy = copy_dir.path().join("mob.sqlite3");
    let copy_name = copy
        .to_str()
        .ok_or_else(|| format!("{} is not a UTF-8 path", copy.display()))?;
    {
        let conn = meerkat_sqlite::open(storage, meerkat_sqlite::ConnectionProfile::ReadOnly)
            .map_err(|error| format!("read-only open failed: {error}"))?;
        conn.execute("VACUUM INTO ?1", [copy_name])
            .map_err(|error| format!("copying it from a read-only connection failed: {error}"))?;
    }
    let copied = meerkat_mob::MobStorage::persistent(&copy)
        .map_err(|error| format!("opening its copy failed: {error}"))?;
    let snapshot = copied
        .created_definition_snapshot()
        .await
        .map_err(|error| format!("reading its copy's definition failed: {error}"))?;
    drop(copied);
    Ok(snapshot.map(|snapshot| snapshot.definition().clone()))
}

/// SHA-256 over a storage's database file and its WAL (absent reads as
/// empty), streamed: the change check for a store whose definition planning
/// read.
fn storage_digest(storage: &Path) -> Result<[u8; 32], String> {
    use std::io::Read as _;
    let mut wal = storage.as_os_str().to_owned();
    wal.push("-wal");
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    for (path, required) in [(storage.to_path_buf(), true), (PathBuf::from(wal), false)] {
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("read {}: {error}", path.display())),
        };
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|error| format!("read {}: {error}", path.display()))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
    }
    Ok(hasher.finalize().into())
}

/// What checking the supplied mob definitions found, by read-only
/// inspection.
#[derive(Debug, Default)]
struct DefinitionCheck {
    ignored_keys: Vec<PredecessorIgnoredKeys>,
    /// `(subject, reason)` of each refusal.
    refusals: Vec<(String, String)>,
    notes: Vec<String>,
    /// Each storage whose definition the check read, with its digest then.
    read: Vec<(PathBuf, [u8; 32])>,
}

impl DefinitionCheck {
    fn record(&self, report: &mut PredecessorAdoptReport) {
        report.ignored_keys.clone_from(&self.ignored_keys);
        report.notes.extend(self.notes.iter().cloned());
        report.errors.extend(
            self.refusals
                .iter()
                .map(|(subject, reason)| format!("refused, nothing changed: {subject}: {reason}")),
        );
    }

    /// Whether every storage the check read still has the bytes it read.
    fn storages_unchanged(&self) -> bool {
        self.read
            .iter()
            .all(|(storage, digest)| storage_digest(storage).is_ok_and(|now| now == *digest))
    }
}

/// Check `check`'s definitions against every persistent mob storage in
/// `state_dir` that a predecessor release which ignored keys created: the
/// release from the store's composition manifest, the definition from the
/// store itself ([`held_definition`]). Changes nothing.
async fn check_definitions(
    state_dir: &Path,
    check: &PredecessorDefinitionCheck,
) -> DefinitionCheck {
    let mut found = DefinitionCheck::default();
    // A directory or manifest the plan cannot judge is refused by the plan.
    let Ok(manifests) = composition_manifests_in(state_dir) else {
        return found;
    };
    // Only a store that a release which ignored keys created can lack one.
    let mut candidates = Vec::new();
    for path in manifests {
        let decoded = regular_file_bytes(&path).and_then(|bytes| {
            serde_json::from_slice::<MobCompositionManifest>(&bytes)
                .map_err(|error| error.to_string())
        });
        let created_by = match decoded {
            Ok(record) => record.created_by_meta,
            Err(reason) => {
                if !check.definitions.is_empty() {
                    found.refusals.push((
                        path.display().to_string(),
                        format!("cannot check the supplied mob definitions against it: {reason}"),
                    ));
                }
                continue;
            }
        };
        match release_version(&created_by) {
            None => found.notes.push(format!(
                "{}: its creator {created_by:?} is not a release version, so the keys it \
                 ignored in the mob definition cannot be told",
                path.display()
            )),
            Some(release) => {
                if let Some(keys) = ignored_keys_of(release) {
                    candidates.push((path, created_by, keys));
                }
            }
        }
    }
    if check.definitions.is_empty() {
        for (path, created_by, keys) in &candidates {
            found.notes.push(format!(
                "{}: predecessor release {created_by} created this store and ignored {keys} in \
                 its mob definition; pass the definition the installation boots with \
                 (--mob-config <mob.toml>) to check it for keys Meta 0.9 applies",
                path.display()
            ));
        }
        return found;
    }
    let mut held = Vec::new();
    for (path, created_by, _) in candidates {
        let Some(storage) = storage_beside(&path) else {
            continue;
        };
        match held_definition(&storage).await {
            Ok(Some(definition)) => match storage_digest(&storage) {
                Ok(digest) => {
                    found.read.push((storage, digest));
                    held.push((path, created_by, definition));
                }
                Err(reason) => found.refusals.push((
                    path.display().to_string(),
                    format!("cannot read the mob storage beside it: {reason}"),
                )),
            },
            Ok(None) => found.notes.push(format!(
                "{}: no mob storage beside it holds a mob, so no definition was compared",
                path.display()
            )),
            Err(reason) => found.refusals.push((
                path.display().to_string(),
                format!(
                    "cannot read the mob definition the store at {} holds: {reason}",
                    storage.display()
                ),
            )),
        }
    }
    let mut checked = vec![false; held.len()];
    for (source, supplied) in &check.definitions {
        let mut matched = false;
        for (index, (path, created_by, definition)) in held.iter().enumerate() {
            if definition.id != supplied.id {
                continue;
            }
            matched = true;
            checked[index] = true;
            let manifest = path.display().to_string();
            for (profile, keys) in profiles_not_held(created_by, definition, supplied) {
                found.notes.push(format!(
                    "{manifest}: profile {profile} in {source} sets {}, which predecessor \
                     release {created_by} ignored, and the definition this store holds has no \
                     profile {profile}: adding the profile changes the store's composition, so \
                     an authoritative resume refuses it as divergent until the stored \
                     definition is advanced, and these keys take effect with it",
                    keys.join(", ")
                ));
            }
            let keys =
                predecessor_ignored_keys(created_by, definition, supplied).unwrap_or_default();
            if keys.is_empty() {
                continue;
            }
            let named = keys.join(", ");
            if check.acknowledge_ignored_keys {
                found.notes.push(format!(
                    "{manifest}: acknowledged keys of the mob definition in {source} that \
                     predecessor release {created_by}, which created this store, ignored and \
                     Meta 0.9 applies, and that the definition this store holds does not \
                     record: {named}. They take effect only once the stored definition is \
                     advanced with the one-shot mob definition update; until then an \
                     authoritative resume that keeps them refuses as divergent"
                ));
            } else {
                found.refusals.push((
                    manifest.clone(),
                    format!(
                        "the mob definition in {source} sets keys that predecessor release \
                         {created_by}, which created this store, ignored and Meta 0.9 applies, \
                         and that the definition this store holds does not record: {named}. \
                         The store has run without them, so an authoritative resume that keeps \
                         them refuses as divergent. Remove them from {source} to keep the \
                         behaviour this store has run with, or rerun with \
                         --acknowledge-ignored-keys and then advance the stored definition once \
                         with the one-shot mob definition update to apply them"
                    ),
                ));
            }
            found.ignored_keys.push(PredecessorIgnoredKeys {
                manifest,
                mob_id: supplied.id.to_string(),
                source: source.clone(),
                created_by: created_by.clone(),
                keys,
                acknowledged: check.acknowledge_ignored_keys,
            });
        }
        if !matched {
            found.notes.push(format!(
                "no persistent mob storage in {} that a release which ignored keys created \
                 holds mob {} ({source}), so nothing in its definition needs checking",
                state_dir.display(),
                supplied.id
            ));
        }
    }
    for ((path, _, definition), checked) in held.iter().zip(checked) {
        if !checked {
            found.notes.push(format!(
                "{}: no supplied mob definition names mob {}, so it was not checked",
                path.display(),
                definition.id
            ));
        }
    }
    found
}

/// Planned state-directory work, computed by read-only inspection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StatePlan {
    /// The predecessor realm directory exists: record its realm id.
    keep_predecessor_realm: bool,
    file_moves: Vec<(PathBuf, PathBuf)>,
    /// The memory ledger file still carries its predecessor name.
    memory_ledger_move: Option<LedgerFileMove>,
    ledgers: Vec<LedgerPlan>,
    /// Composition manifests whose creator field still has its predecessor
    /// spelling.
    composition_manifests: Vec<ManifestRename>,
    schedules: Vec<String>,
}

/// One composition manifest whose creator field still has its predecessor
/// spelling: renamed in place, every other byte kept. Its bytes keep their
/// digest from planning to the rename.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestRename {
    path: PathBuf,
    /// SHA-256 of the bytes the plan saw.
    digest: [u8; 32],
}

/// The memory ledger file's rename: its committed bytes keep their digest
/// from planning to the rename, and the destination stays free.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LedgerFileMove {
    from: PathBuf,
    to: PathBuf,
    /// SHA-256 of the committed bytes the plan saw.
    digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LedgerPlan {
    database: PathBuf,
    renames: Vec<(&'static str, &'static str)>,
    moves_table: bool,
}

/// Planned gateway-home work, computed by read-only inspection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct HomePlan {
    /// `(predecessor, home)` when the predecessor home still has to move.
    home_move: Option<(PathBuf, PathBuf)>,
    /// `(from, to)` when the draft store still carries its predecessor name.
    draft_rename: Option<(PathBuf, PathBuf)>,
    /// The established identity that must survive the move.
    identity: Option<EstablishedIdentity>,
}

/// The peer identity an installation has established: its public key and
/// the host pins and audience bindings recorded against it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EstablishedIdentity {
    pubkey_b64: Option<String>,
    pairings: Vec<(String, crate::runtime::remote_host::HostPairingRecord)>,
}

/// Run one adoption pass over one installation.
///
/// Custody precondition: every process using the installation is stopped.
/// The run enforces what it can observe, and no more:
///
/// - It refuses while the runtime registry of the state directory or either
///   home lists a live process, before fencing and again under the fence.
/// - Apply holds the exclusive maintenance fence over every database it
///   reads or writes (existing databases, predecessor sources and renamed
///   destinations) from before revalidation until the gateway-home steps
///   have finished. The fence covers SQLite databases only. Nothing fences
///   the memory ledger's JSON writer or the composition manifests: the
///   precondition is what keeps writers away, and revalidating each file's
///   digest detects a change since planning without excluding one.
/// - Each rename is followed by an fsync of the directories it touched on
///   Unix only; elsewhere its durability rests on the filesystem's own
///   ordering.
///
/// `request.state_dir` is used as given: it must be the directory the
/// installation's gateway opens (the one its startup refusal names), and
/// nothing here searches for another.
///
/// Steps run in order and the run stops at the first failure, so no step
/// runs after a failed prerequisite.
pub async fn adopt_predecessor_state(request: &PredecessorAdoptRequest) -> PredecessorAdoptReport {
    adopt_predecessor_state_with_definitions(request, &PredecessorDefinitionCheck::default()).await
}

/// [`adopt_predecessor_state`], also checking the mob definitions the
/// installation boots with ([`PredecessorDefinitionCheck`]). Planning names
/// each key that the predecessor release which created a store ignored,
/// that Meta 0.9 applies and that the definition the store holds does not
/// record ([`PredecessorAdoptReport::ignored_keys`]), and refuses before any
/// change unless the check acknowledges them. Without definitions it notes
/// each store whose creating release ignored such keys.
pub async fn adopt_predecessor_state_with_definitions(
    request: &PredecessorAdoptRequest,
    check: &PredecessorDefinitionCheck,
) -> PredecessorAdoptReport {
    let mut report = PredecessorAdoptReport {
        mode: request.mode,
        state_dir: request.state_dir.clone(),
        ..PredecessorAdoptReport::default()
    };
    let state_dir = request.state_dir.as_path();
    if !state_dir.is_dir() {
        report.refuse(
            PredecessorItem::Custody,
            state_dir.display().to_string(),
            "state directory does not exist",
        );
        return report;
    }
    let journal = match read_adoption_journal(state_dir) {
        Ok(journal) => journal,
        Err(detail) => {
            report.refuse(
                PredecessorItem::Journal,
                state_dir.display().to_string(),
                crate::storage_layout::StorageLayoutError::AdoptionJournalUnreadable {
                    path: state_dir.join(ADOPTION_JOURNAL_FILE_NAME),
                    detail,
                }
                .to_string(),
            );
            return report;
        }
    };
    // A staged journal from an earlier run is evidence this run must not
    // overwrite or remove.
    let staged = staged_journal_path(state_dir);
    if staged.exists() {
        report.refuse(
            PredecessorItem::Journal,
            staged.display().to_string(),
            JournalWriteError::StagePresent {
                staged: staged.clone(),
            }
            .to_string(),
        );
        return report;
    }

    // The home scope: an unfinished adoption is bound to the homes its
    // journal recorded; otherwise the request's.
    let requested = HomeScope {
        predecessor_home: request.predecessor_home.clone(),
        gateway_home: request.gateway_home.clone(),
    };
    let scope = match &journal {
        Some(journal) if journal.status == AdoptionStatus::InProgress => {
            let recorded = HomeScope {
                predecessor_home: journal.predecessor_home.clone(),
                gateway_home: journal.gateway_home.clone(),
            };
            if request.homes_explicit && requested != recorded {
                report.refuse(
                    PredecessorItem::GatewayHome,
                    "home scope",
                    format!(
                        "the unfinished adoption recorded {}; the supplied {} conflicts with it. \
                         Rerun with the recorded homes, or omit them",
                        recorded.describe(),
                        requested.describe()
                    ),
                );
                return report;
            }
            report.notes.push(format!(
                "resuming an unfinished adoption with its recorded {}; finished steps: {:?}",
                recorded.describe(),
                journal.completed
            ));
            recorded
        }
        _ => requested,
    };
    if scope.predecessor_home.is_some() && scope.gateway_home.is_none() {
        report.refuse(
            PredecessorItem::GatewayHome,
            "gateway home",
            "a predecessor home needs the gateway home it becomes",
        );
        return report;
    }

    // Plan by read-only inspection. Every refusal here happens before any
    // change.
    refuse_live_runtimes(state_dir, &scope, &mut report);
    let state_plan = plan_state(state_dir, &mut report);
    let home_plan = plan_home(&scope, &mut report);
    let definitions = check_definitions(state_dir, check).await;
    definitions.record(&mut report);
    if report.has_errors() {
        return report;
    }
    let (Some(state_plan), Some(home_plan)) = (state_plan, home_plan) else {
        return report;
    };
    let realm_recorded = journal
        .as_ref()
        .is_some_and(|journal| journal.realm_id.as_deref() == Some(PREDECESSOR_REALM_ID));
    // Any planned change defeats the already-complete shortcut, including
    // predecessor state found after an earlier adoption completed (a memory
    // ledger file, for one).
    let nothing_to_do = StatePlan {
        keep_predecessor_realm: state_plan.keep_predecessor_realm && !realm_recorded,
        ..state_plan.clone()
    } == StatePlan::default()
        && home_plan.home_move.is_none()
        && home_plan.draft_rename.is_none();
    let journal_complete = journal
        .as_ref()
        .is_some_and(|journal| journal.status == AdoptionStatus::Complete);
    let apply = request.mode == MigrateMode::Apply;
    if !apply {
        record_plan(
            &state_plan,
            &home_plan,
            !realm_recorded,
            PredecessorOutcome::Planned,
            &mut report,
        );
        if report.actions.is_empty() {
            report
                .notes
                .push("no predecessor state found; nothing to adopt".into());
        }
        report.complete = journal_complete && nothing_to_do;
        return report;
    }

    // Custody: fence every database the run touches, then reread the
    // journal and revalidate the plan under the fence before the first
    // change.
    let Some(_fences) = acquire_fences(state_dir, &state_plan, &mut report) else {
        return report;
    };
    let journal_unchanged =
        read_adoption_journal(state_dir).is_ok_and(|reread| reread == journal) && !staged.exists();
    refuse_live_runtimes(state_dir, &scope, &mut report);
    let revalidated_state = plan_state(state_dir, &mut report);
    let revalidated_home = plan_home(&scope, &mut report);
    if report.has_errors() {
        return report;
    }
    if !journal_unchanged
        || revalidated_state.as_ref() != Some(&state_plan)
        || revalidated_home.as_ref() != Some(&home_plan)
        || !definitions.storages_unchanged()
    {
        report.refuse(
            PredecessorItem::Custody,
            state_dir.display().to_string(),
            "the installation changed between planning and fencing; another process is using it",
        );
        return report;
    }

    let mut journal = journal.unwrap_or(AdoptionJournal {
        version: ADOPTION_JOURNAL_VERSION,
        status: AdoptionStatus::InProgress,
        predecessor_home: None,
        gateway_home: None,
        completed: BTreeSet::new(),
        realm_id: None,
    });
    if nothing_to_do && journal_complete {
        report.complete = true;
        report
            .notes
            .push("adoption already complete; nothing to do".into());
        return report;
    }
    // Reopening a finished adoption for predecessor state found since: no
    // step of this run has finished yet, so the journal records none, and a
    // failure names the steps this run still owes.
    if journal_complete {
        journal.completed.clear();
    }
    journal.status = AdoptionStatus::InProgress;
    journal.predecessor_home = scope.predecessor_home.clone();
    journal.gateway_home = scope.gateway_home.clone();
    // The realm id is recorded configuration, written with the journal
    // before any data change; nothing is rewritten under it.
    let record_realm_now = state_plan.keep_predecessor_realm && !realm_recorded;
    if record_realm_now {
        journal.realm_id = Some(PREDECESSOR_REALM_ID.to_string());
    }
    match write_adoption_journal(state_dir, &journal) {
        Ok(()) => {}
        Err(
            error @ (JournalWriteError::StagePresent { .. }
            | JournalWriteError::Staging {
                left_behind: false, ..
            }),
        ) => {
            report.refuse(
                PredecessorItem::Journal,
                state_dir.display().to_string(),
                error.to_string(),
            );
            return report;
        }
        Err(error) => {
            // A staged file left behind, or a replaced journal that is not
            // yet durable: this run changed the directory.
            report.changed_state = true;
            report.record(
                PredecessorItem::Journal,
                state_dir.display().to_string(),
                "",
                "",
                PredecessorOutcome::Failed(error.to_string()),
            );
            return report;
        }
    }
    report.changed_state = true;
    if record_realm_now {
        record_realm(PredecessorOutcome::Applied, &mut report);
    }

    for step in AdoptionStep::ALL {
        let finished = match step {
            AdoptionStep::DatabaseFiles => apply_file_moves(&state_plan, &mut report),
            AdoptionStep::MemoryLedgerFile => apply_memory_ledger_move(&state_plan, &mut report),
            AdoptionStep::Ledger => {
                apply_ledgers(&state_plan, &mut report)
                    && apply_composition_manifests(&state_plan, &mut report)
            }
            AdoptionStep::StewardSchedule => {
                apply_schedules(state_dir, &state_plan, &mut report).await
            }
            AdoptionStep::GatewayHome => apply_home_move(&home_plan, &mut report),
            AdoptionStep::DraftStore => apply_draft_rename(&home_plan, &mut report),
        };
        if !finished {
            // Stop: no dependent step runs after a failed prerequisite.
            let _ = write_adoption_journal(state_dir, &journal);
            return report;
        }
        journal.completed.insert(step);
        if let Err(error) = write_adoption_journal(state_dir, &journal) {
            report.record(
                PredecessorItem::Journal,
                state_dir.display().to_string(),
                "",
                "",
                PredecessorOutcome::Failed(error.to_string()),
            );
            return report;
        }
    }
    journal.status = AdoptionStatus::Complete;
    match write_adoption_journal(state_dir, &journal) {
        Ok(()) => report.complete = true,
        Err(error) => report.record(
            PredecessorItem::Journal,
            state_dir.display().to_string(),
            "",
            "",
            PredecessorOutcome::Failed(error.to_string()),
        ),
    }
    report
}

/// Record the dry run's plan. `record_realm_id` is false once the journal
/// already records the predecessor realm id: there is nothing left to plan
/// for it.
fn record_plan(
    state_plan: &StatePlan,
    home_plan: &HomePlan,
    record_realm_id: bool,
    outcome: PredecessorOutcome,
    report: &mut PredecessorAdoptReport,
) {
    if state_plan.keep_predecessor_realm && record_realm_id {
        record_realm(outcome.clone(), report);
    }
    for (from, to) in &state_plan.file_moves {
        report.record(
            PredecessorItem::DatabaseFile,
            from.display().to_string(),
            from.display().to_string(),
            to.display().to_string(),
            outcome.clone(),
        );
    }
    if let Some(planned) = &state_plan.memory_ledger_move {
        record_memory_ledger(planned, outcome.clone(), report);
    }
    for ledger in &state_plan.ledgers {
        record_ledger(ledger, outcome.clone(), report);
    }
    for planned in &state_plan.composition_manifests {
        record_composition_manifest(planned, outcome.clone(), report);
    }
    for schedule_id in &state_plan.schedules {
        report.record(
            PredecessorItem::StewardSchedule,
            format!("schedule {schedule_id}"),
            PREDECESSOR_STEWARD_DREAM_RUNNABLE,
            crate::schedule_wiring::STEWARD_DREAM_RUNNABLE,
            outcome.clone(),
        );
    }
    if let Some((from, to)) = &home_plan.home_move {
        report.record(
            PredecessorItem::GatewayHome,
            from.display().to_string(),
            from.display().to_string(),
            to.display().to_string(),
            outcome.clone(),
        );
    }
    if let Some((from, to)) = &home_plan.draft_rename {
        report.record(
            PredecessorItem::DraftStore,
            from.display().to_string(),
            from.display().to_string(),
            to.display().to_string(),
            outcome,
        );
    }
}

fn record_realm(outcome: PredecessorOutcome, report: &mut PredecessorAdoptReport) {
    report.record(
        PredecessorItem::RealmId,
        "meerkat-level realm id",
        PREDECESSOR_REALM_ID,
        format!("{PREDECESSOR_REALM_ID} (recorded in {ADOPTION_JOURNAL_FILE_NAME})"),
        outcome,
    );
}

fn record_memory_ledger(
    planned: &LedgerFileMove,
    outcome: PredecessorOutcome,
    report: &mut PredecessorAdoptReport,
) {
    report.record(
        PredecessorItem::MemoryLedger,
        planned.from.display().to_string(),
        planned.from.display().to_string(),
        planned.to.display().to_string(),
        outcome,
    );
}

fn record_composition_manifest(
    planned: &ManifestRename,
    outcome: PredecessorOutcome,
    report: &mut PredecessorAdoptReport,
) {
    report.record(
        PredecessorItem::CompositionManifest,
        planned.path.display().to_string(),
        PREDECESSOR_MANIFEST_CREATOR_FIELD,
        MANIFEST_CREATOR_FIELD,
        outcome,
    );
}

fn record_ledger(
    ledger: &LedgerPlan,
    outcome: PredecessorOutcome,
    report: &mut PredecessorAdoptReport,
) {
    let subject = ledger.database.display().to_string();
    for (from, to) in &ledger.renames {
        report.record(
            PredecessorItem::LedgerDomain,
            subject.clone(),
            *from,
            *to,
            outcome.clone(),
        );
    }
    if ledger.moves_table {
        report.record(
            PredecessorItem::MetadataTable,
            subject,
            PREDECESSOR_METADATA_TABLE,
            crate::runtime::METADATA_TABLE,
            outcome,
        );
    }
}

/// Refuse while a runtime registry the installation owns lists a live
/// process.
fn refuse_live_runtimes(state_dir: &Path, scope: &HomeScope, report: &mut PredecessorAdoptReport) {
    let roots = std::iter::once(state_dir)
        .chain(scope.predecessor_home.as_deref())
        .chain(scope.gateway_home.as_deref());
    for root in roots {
        let registry = root.join(crate::storage_layout::RUNTIME_REGISTRY_FILE_NAME);
        let entries = match crate::storage_migrate::registered_runtime_liveness(&registry) {
            Ok(entries) => entries,
            Err(error) => {
                report.refuse(
                    PredecessorItem::Custody,
                    registry.display().to_string(),
                    format!("cannot prove the installation is stopped: {error}"),
                );
                continue;
            }
        };
        for (runtime_id, pid, alive) in entries {
            match alive {
                Some(false) => {}
                Some(true) => report.refuse(
                    PredecessorItem::Custody,
                    registry.display().to_string(),
                    format!("runtime '{runtime_id}' (pid {pid}) is still running; stop it first"),
                ),
                None => report.refuse(
                    PredecessorItem::Custody,
                    registry.display().to_string(),
                    format!(
                        "cannot prove runtime '{runtime_id}' (pid {pid}) is stopped on this \
                         platform"
                    ),
                ),
            }
        }
    }
}

/// Plan the state-directory steps by read-only inspection. `None` (with a
/// refusal recorded) when the installation cannot be adopted as it lies.
fn plan_state(state_dir: &Path, report: &mut PredecessorAdoptReport) -> Option<StatePlan> {
    let errors_before = report.errors.len();
    let mut plan = StatePlan::default();
    let predecessor_realm = state_dir.join(PREDECESSOR_REALM_ID);
    let default_realm = state_dir.join(crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID);
    if predecessor_realm.exists() && default_realm.exists() {
        report.refuse(
            PredecessorItem::RealmId,
            state_dir.display().to_string(),
            format!(
                "both realm directories {} and {} exist; refusing to pick one",
                predecessor_realm.display(),
                default_realm.display()
            ),
        );
    } else if predecessor_realm.is_dir() {
        plan.keep_predecessor_realm = true;
    }
    for (names, canonical) in [
        (
            &PREDECESSOR_METADATA_DB_FILE_NAMES,
            CANONICAL_METADATA_DB_FILE_NAME,
        ),
        (
            &PREDECESSOR_CONSOLE_DB_FILE_NAMES,
            CANONICAL_CONSOLE_DB_FILE_NAME,
        ),
    ] {
        let existing: Vec<PathBuf> = names
            .iter()
            .map(|name| state_dir.join(name))
            .filter(|path| path.is_file())
            .collect();
        let target = state_dir.join(canonical);
        match existing.as_slice() {
            [] => {}
            [from] if !target.exists() => plan.file_moves.push((from.clone(), target)),
            [from] => report.refuse(
                PredecessorItem::DatabaseFile,
                from.display().to_string(),
                format!(
                    "the renamed file {} already exists beside its predecessor; refusing to \
                     merge two histories (inspect both, keep one)",
                    target.display()
                ),
            ),
            many => report.refuse(
                PredecessorItem::DatabaseFile,
                display_paths(many),
                "two predecessor spellings of the same store exist; resolve them with the \
                 predecessor release's storage-migrate first",
            ),
        }
    }

    match plan_memory_ledger(state_dir) {
        Ok(planned) => plan.memory_ledger_move = planned,
        Err(reason) => report.refuse(
            PredecessorItem::MemoryLedger,
            state_dir
                .join(PREDECESSOR_MEMORY_LEDGER_FILE_NAME)
                .display()
                .to_string(),
            reason,
        ),
    }

    match composition_manifests_in(state_dir) {
        Ok(manifests) => {
            for manifest in manifests {
                match plan_composition_manifest(&manifest) {
                    Ok(Some(planned)) => plan.composition_manifests.push(planned),
                    Ok(None) => {}
                    Err(reason) => report.refuse(
                        PredecessorItem::CompositionManifest,
                        manifest.display().to_string(),
                        reason,
                    ),
                }
            }
        }
        Err(reason) => report.refuse(
            PredecessorItem::CompositionManifest,
            state_dir.display().to_string(),
            reason,
        ),
    }

    let mut databases = enumerate_state_dir_databases(state_dir);
    databases.extend(plan.file_moves.iter().map(|(from, _)| from.clone()));
    databases.sort();
    databases.dedup();
    for database in &databases {
        match plan_ledger(database) {
            Ok(Some(ledger)) => {
                // The plan names the database where it will lie once the
                // file step has run.
                let database = plan
                    .file_moves
                    .iter()
                    .find(|(from, _)| from == database)
                    .map_or_else(|| database.clone(), |(_, to)| to.clone());
                plan.ledgers.push(LedgerPlan { database, ..ledger });
            }
            Ok(None) => {}
            Err(reason) => report.refuse(
                PredecessorItem::LedgerDomain,
                database.display().to_string(),
                reason,
            ),
        }
    }

    let schedule_store =
        MetaStorageLayout::with_injected_roots(state_dir.to_path_buf(), None).schedule_db();
    if schedule_store.is_file() {
        match plan_schedules(&schedule_store) {
            Ok(ids) => plan.schedules = ids,
            Err(reason) => report.refuse(
                PredecessorItem::StewardSchedule,
                schedule_store.display().to_string(),
                reason,
            ),
        }
    }
    (report.errors.len() == errors_before).then_some(plan)
}

/// Read-only inspection of one database's ledger.
fn plan_ledger(database: &Path) -> Result<Option<LedgerPlan>, String> {
    let conn = meerkat_sqlite::open(database, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(|error| format!("read-only open failed: {error}"))?;
    let Some(domains) = read_ledger_domains(&conn).map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    let renames: Vec<(&'static str, &'static str)> = PREDECESSOR_LEDGER_DOMAINS
        .iter()
        .copied()
        .chain([(PREDECESSOR_EXTENSION_LEDGER_DOMAIN, EXTENSION_LEDGER_DOMAIN)])
        .filter(|(from, _)| domains.iter().any(|domain| domain == from))
        .collect();
    if renames.is_empty() {
        return Ok(None);
    }
    if let Some((from, to)) = renames
        .iter()
        .find(|(_, to)| domains.iter().any(|domain| domain == to))
    {
        return Err(format!(
            "the database carries both ledger domains {from} and {to}; refusing to pick one"
        ));
    }
    let moves_table = renames.iter().any(|(from, _)| *from == "mobkit-metadata");
    if moves_table && table_exists(&conn, crate::runtime::METADATA_TABLE).unwrap_or(true) {
        return Err(
            "the renamed metadata table already exists; refusing to merge two tables".into(),
        );
    }
    Ok(Some(LedgerPlan {
        database: database.to_path_buf(),
        renames,
        moves_table,
    }))
}

/// Read-only inspection of the stock gateway's memory ledger file at its one
/// fixed path in the state directory: nothing is searched for, and a ledger
/// an embedder configured at another `state_path` is never touched.
///
/// The stage an interrupted predecessor write left behind is evidence: it is
/// preserved and refused for manual resolution, never deleted or promoted.
/// A renamed ledger (or its stage) beside the predecessor file refuses as
/// twins: one history is never merged into or written over another.
fn plan_memory_ledger(state_dir: &Path) -> Result<Option<LedgerFileMove>, String> {
    let from = state_dir.join(PREDECESSOR_MEMORY_LEDGER_FILE_NAME);
    let stage = crate::runtime::staged_ledger_path(&from);
    if present(&stage) {
        return Err(format!(
            "{} is the stage of an interrupted ledger write and may hold its newest state; it \
             is preserved, never deleted or promoted here. Inspect it: if it decodes as a \
             complete ledger newer than {}, move it over that file, otherwise remove it; then \
             rerun",
            stage.display(),
            from.display()
        ));
    }
    if !present(&from) {
        return Ok(None);
    }
    let to = state_dir.join(crate::storage_layout::MEMORY_LEDGER_FILE_NAME);
    if let Some(occupied) = ledger_names(&to).find(|path| present(path)) {
        return Err(format!(
            "the renamed ledger {} already exists beside its predecessor; refusing to merge two \
             histories (inspect both, keep one)",
            occupied.display()
        ));
    }
    Ok(Some(LedgerFileMove {
        digest: ledger_digest(&from)?,
        from,
        to,
    }))
}

/// A ledger file and the stage its writer goes through.
fn ledger_names(path: &Path) -> impl Iterator<Item = PathBuf> {
    [path.to_path_buf(), crate::runtime::staged_ledger_path(path)].into_iter()
}

/// SHA-256 of a ledger file's committed bytes. Anything but a regular file
/// refuses: a link or a directory at the ledger's name is resolved by hand.
fn ledger_digest(path: &Path) -> Result<[u8; 32], String> {
    regular_file_bytes(path).map(|bytes| Sha256::digest(bytes).into())
}

/// The bytes of the regular file at `path`. Anything else refuses: a link or
/// a directory at a name adoption rewrites is resolved by hand.
fn regular_file_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("inspect {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "{} is not a regular file; resolve it by hand",
            path.display()
        ));
    }
    std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))
}

/// Every composition manifest directly in `dir`: each entry whose name ends
/// in [`crate::mob_composition_manifest::MANIFEST_FILE_SUFFIX`], sorted.
/// Subdirectories are not searched: a persistent mob storage keeps its
/// manifest beside it, and the stock one is the event log in the state
/// directory itself.
pub fn composition_manifests_in(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|error| format!("list {}: {error}", dir.display()))?;
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("list {}: {error}", dir.display()))?;
        if entry.file_name().to_str().is_some_and(|name| {
            name.ends_with(crate::mob_composition_manifest::MANIFEST_FILE_SUFFIX)
        }) {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}

/// How a composition manifest spells its creator field.
enum ManifestCreator {
    /// `created_by_meta`: nothing to rename.
    Current,
    /// [`PREDECESSOR_MANIFEST_CREATOR_FIELD`], at this byte range of its
    /// quoted key.
    Predecessor(std::ops::Range<usize>),
}

/// Read-only inspection of one composition manifest: `Some` when its creator
/// field still has its predecessor spelling. Refused for manual resolution,
/// before any change: a manifest that is not a regular file or not a JSON
/// object, one that names no creator field or more than one, and the stage
/// of an interrupted rename beside it.
fn plan_composition_manifest(path: &Path) -> Result<Option<ManifestRename>, String> {
    let stage = staged_manifest_path(path);
    if present(&stage) {
        return Err(format!(
            "{} is the stage of an interrupted manifest rename; the manifest itself is intact \
             (the rename replaces it in one step). Remove the stage, then rerun",
            stage.display()
        ));
    }
    let bytes = regular_file_bytes(path)?;
    match manifest_creator(&bytes).map_err(|reason| format!("{}: {reason}", path.display()))? {
        ManifestCreator::Current => Ok(None),
        ManifestCreator::Predecessor(_) => Ok(Some(ManifestRename {
            path: path.to_path_buf(),
            digest: Sha256::digest(&bytes).into(),
        })),
    }
}

/// Which creator field the manifest `bytes` carry, judged on the top-level
/// keys only: a key of the same name nested inside the recorded definition
/// is data, not the creator field.
fn manifest_creator(bytes: &[u8]) -> Result<ManifestCreator, String> {
    let keys = top_level_keys(bytes).ok_or(
        "not a JSON object, so not a readable composition manifest; restore it (startup \
         refuses it as malformed too)",
    )?;
    let mut predecessor = keys
        .iter()
        .filter(|(key, _)| key == PREDECESSOR_MANIFEST_CREATOR_FIELD)
        .map(|(_, span)| span.clone());
    let current = keys
        .iter()
        .filter(|(key, _)| key == MANIFEST_CREATOR_FIELD)
        .count();
    match (predecessor.next(), predecessor.next(), current) {
        (None, _, 1) => Ok(ManifestCreator::Current),
        (Some(span), None, 0) => Ok(ManifestCreator::Predecessor(span)),
        (None, _, 0) => Err(format!(
            "names no creator field ({MANIFEST_CREATOR_FIELD} or \
             {PREDECESSOR_MANIFEST_CREATOR_FIELD}), so it is not a manifest this build can \
             judge; restore it"
        )),
        _ => Err(format!(
            "carries more than one creator field ({MANIFEST_CREATOR_FIELD} and/or \
             {PREDECESSOR_MANIFEST_CREATOR_FIELD}); refusing to pick one (inspect it and keep \
             the one naming the release that created the store)"
        )),
    }
}

/// The top-level keys of the JSON object `bytes`, each with the byte range
/// of its quoted key, in document order; `None` unless `bytes` is one JSON
/// object. Duplicate keys are all listed (a decoder would keep only one).
fn top_level_keys(bytes: &[u8]) -> Option<Vec<(String, std::ops::Range<usize>)>> {
    // Validated first, so the scan below only ever sees well-formed JSON:
    // structural characters inside strings are skipped with the strings,
    // and no byte of a multi-byte UTF-8 sequence is a quote or a backslash.
    if !serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|value| value.is_object()) {
        return None;
    }
    let mut keys = Vec::new();
    let mut depth = 0_usize;
    let mut expect_key = false;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let start = index;
                index += 1;
                while index < bytes.len() && bytes[index] != b'"' {
                    index += if bytes[index] == b'\\' { 2 } else { 1 };
                }
                let end = index + 1;
                if depth == 1 && expect_key {
                    keys.push((
                        serde_json::from_slice::<String>(bytes.get(start..end)?).ok()?,
                        start..end,
                    ));
                }
                expect_key = false;
                index = end;
                continue;
            }
            b'{' | b'[' => {
                depth += 1;
                expect_key = depth == 1;
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            b',' => expect_key = depth == 1,
            _ => {}
        }
        index += 1;
    }
    Some(keys)
}

/// `bytes` with the predecessor creator key at `key` renamed, every other
/// byte kept, proven to decode to the same document with the key moved.
fn renamed_manifest(bytes: &[u8], key: std::ops::Range<usize>) -> Result<Vec<u8>, String> {
    let replacement =
        serde_json::to_vec(MANIFEST_CREATOR_FIELD).map_err(|error| error.to_string())?;
    let (Some(before), Some(after)) = (bytes.get(..key.start), bytes.get(key.end..)) else {
        return Err("the creator key lies outside the manifest".to_string());
    };
    let renamed = [before, replacement.as_slice(), after].concat();
    let mut expected: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let moved = expected
        .as_object_mut()
        .and_then(|object| {
            let value = object.remove(PREDECESSOR_MANIFEST_CREATOR_FIELD)?;
            object.insert(MANIFEST_CREATOR_FIELD.to_string(), value);
            Some(())
        })
        .is_some();
    let actual: Option<serde_json::Value> = serde_json::from_slice(&renamed).ok();
    if !moved || actual.as_ref() != Some(&expected) {
        return Err(
            "renaming the creator key would change more than the key; nothing was written"
                .to_string(),
        );
    }
    Ok(renamed)
}

/// The staged sibling a manifest rename goes through.
fn staged_manifest_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(".next");
    path.with_file_name(name)
}

/// Read-only inspection of the schedule store: the ids of every live
/// schedule that targets the predecessor steward runnable. Never opens the
/// store through its owner, which would apply migrations.
///
/// This is also the schedule step's preflight, run before the fence and
/// again under it, before the first change: `schedule_json` is read as TEXT
/// or BLOB (carried stores hold both); every row must decode as the typed
/// schedule the service reads and carry its own row id; a deleted schedule
/// is retired and left as it is (the schedule service never sees it). These
/// schedule decode and identity faults are refused here, before the first
/// mutation; a failure in a later step after changes have begun stays
/// resumable from the journal.
fn plan_schedules(store_path: &Path) -> Result<Vec<String>, String> {
    let conn = meerkat_sqlite::open(store_path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(|error| format!("read-only open failed: {error}"))?;
    if !table_exists(&conn, "schedule_schedules").map_err(|error| error.to_string())? {
        return Ok(Vec::new());
    }
    let mut statement = conn
        .prepare("SELECT schedule_id, schedule_json FROM schedule_schedules ORDER BY schedule_id")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, meerkat_sqlite::JsonColumnBytes>(1)?
                    .into_bytes(),
            ))
        })
        .map_err(|error| error.to_string())?;
    let mut ids = Vec::new();
    for row in rows {
        let (schedule_id, bytes) = row.map_err(|error| error.to_string())?;
        // Every row decodes as the typed schedule the service reads, and
        // names itself: a row the service could not read refuses here.
        let schedule: meerkat::Schedule = serde_json::from_slice(&bytes)
            .map_err(|error| format!("schedule {schedule_id} is not a valid schedule: {error}"))?;
        if schedule.schedule_id.to_string() != schedule_id {
            return Err(format!(
                "schedule row {schedule_id} holds schedule {}; refusing to retarget a \
                 mismatched row",
                schedule.schedule_id
            ));
        }
        if schedule.phase == meerkat::SchedulePhase::Deleted {
            continue;
        }
        if matches!(
            &schedule.target,
            TargetBinding::HostRunnable(binding)
                if binding.runnable.as_str() == PREDECESSOR_STEWARD_DREAM_RUNNABLE
        ) {
            ids.push(schedule_id);
        }
    }
    Ok(ids)
}

/// Plan the gateway-home steps by read-only inspection, resuming from
/// wherever an interrupted run left the home.
fn plan_home(scope: &HomeScope, report: &mut PredecessorAdoptReport) -> Option<HomePlan> {
    let errors_before = report.errors.len();
    let mut plan = HomePlan::default();
    let Some(home) = scope.gateway_home.as_deref() else {
        return Some(plan);
    };
    let predecessor = scope
        .predecessor_home
        .as_deref()
        .filter(|path| path.exists());
    let identity_root = match predecessor {
        Some(predecessor) => {
            let occupied = home.is_file()
                || (home.is_dir()
                    && std::fs::read_dir(home)
                        .map(|mut entries| entries.next().is_some())
                        .unwrap_or(true));
            if occupied {
                report.refuse(
                    PredecessorItem::GatewayHome,
                    predecessor.display().to_string(),
                    format!(
                        "the renamed gateway home {} already exists and is not empty; \
                         refusing to merge two peer identities (inspect both, keep one)",
                        home.display()
                    ),
                );
                return None;
            }
            plan.home_move = Some((predecessor.to_path_buf(), home.to_path_buf()));
            predecessor
        }
        None => home,
    };
    if identity_root.is_dir() {
        match established_identity(identity_root) {
            Ok(identity) => plan.identity = Some(identity),
            Err(reason) => report.refuse(
                PredecessorItem::PeerIdentity,
                identity_root.display().to_string(),
                reason,
            ),
        }
        if identity_root
            .join(PREDECESSOR_DRAFT_STORE_FILE_NAME)
            .is_file()
        {
            let drafts_to = home.join(crate::mobpack::MOBPACK_DRAFT_STORE_FILENAME);
            if identity_root
                .join(crate::mobpack::MOBPACK_DRAFT_STORE_FILENAME)
                .exists()
            {
                report.refuse(
                    PredecessorItem::DraftStore,
                    identity_root.display().to_string(),
                    "both draft store names exist; refusing to merge two draft stores",
                );
            } else {
                plan.draft_rename = Some((home.join(PREDECESSOR_DRAFT_STORE_FILE_NAME), drafts_to));
            }
        }
    }
    (report.errors.len() == errors_before).then_some(plan)
}

/// Load and validate the identity an installation established at `root`:
/// a loadable peer key, and pairings (host pins and control audiences) that
/// decode. An installation that recorded pairings without a usable key is
/// refused: adoption never mints a replacement identity.
fn established_identity(root: &Path) -> Result<EstablishedIdentity, String> {
    let pairings_path = root.join(crate::runtime::remote_host::HOST_PAIRING_FILE_NAME);
    let pairings: Vec<(String, crate::runtime::remote_host::HostPairingRecord)> =
        crate::runtime::remote_host::HostPairingStore::load(&pairings_path)
            .map_err(|error| format!("host pairings do not load: {error}"))?
            .hosts()
            .map(|(key, record)| (key.to_string(), record.clone()))
            .collect();
    let key_path = root.join(crate::auth::peer_keys::KEY_FILE_NAME);
    let pubkey_b64 = if key_path.exists() {
        Some(
            crate::GatewayPeerKeys::load_existing(root)
                .map_err(|error| format!("peer key does not load: {error}"))?
                .pubkey_b64(),
        )
    } else if pairings_path.exists() {
        return Err(
            "the installation recorded host pairings but has no peer key; restore the \
             established key (adoption never mints a replacement)"
                .into(),
        );
    } else {
        None
    };
    Ok(EstablishedIdentity {
        pubkey_b64,
        pairings,
    })
}

/// Take the exclusive fence over every database the run touches:
/// existing databases, predecessor sources and the renamed destinations
/// (an absent destination is fenced through its sidecar). Any failure is a
/// refusal before change.
fn acquire_fences(
    state_dir: &Path,
    plan: &StatePlan,
    report: &mut PredecessorAdoptReport,
) -> Option<(MetaMaintenanceFence, Vec<meerkat_sqlite::ExclusiveFence>)> {
    let fence = match MetaMaintenanceFence::acquire(state_dir, FENCE_DRAIN_DEADLINE) {
        Ok(fence) => fence,
        Err(error) => {
            report.refuse(
                PredecessorItem::Custody,
                state_dir.display().to_string(),
                format!("maintenance fence not acquirable: {error}"),
            );
            return None;
        }
    };
    let mut extra = Vec::new();
    for (from, to) in &plan.file_moves {
        match meerkat_sqlite::ExclusiveFence::acquire(from, FENCE_DRAIN_DEADLINE) {
            Ok(acquired) => extra.push(acquired),
            Err(error) => {
                report.refuse(
                    PredecessorItem::Custody,
                    from.display().to_string(),
                    format!("maintenance fence not acquirable: {error}"),
                );
                return None;
            }
        }
        match meerkat_sqlite::ExclusiveFence::try_acquire(to) {
            Ok(Some(acquired)) => extra.push(acquired),
            Ok(None) => {
                report.refuse(
                    PredecessorItem::Custody,
                    to.display().to_string(),
                    "the renamed destination is held by another process",
                );
                return None;
            }
            Err(error) => {
                report.refuse(
                    PredecessorItem::Custody,
                    to.display().to_string(),
                    format!("the renamed destination cannot be fenced: {error}"),
                );
                return None;
            }
        }
    }
    Some((fence, extra))
}

fn apply_file_moves(plan: &StatePlan, report: &mut PredecessorAdoptReport) -> bool {
    for (from, to) in &plan.file_moves {
        let subject = from.display().to_string();
        // The move, then an fsync of the directory it happened in: the
        // journal records the step finished only once the rename is durable.
        match move_database_file(from, to).and_then(|_| sync_parent(to)) {
            Ok(()) => {
                if let Err(error) = write_rename_marker(from, to) {
                    report
                        .notes
                        .push(format!("rename marker for {subject} not written: {error}"));
                }
                report.record(
                    PredecessorItem::DatabaseFile,
                    subject,
                    from.display().to_string(),
                    to.display().to_string(),
                    PredecessorOutcome::Applied,
                );
            }
            Err(error) => {
                report.record(
                    PredecessorItem::DatabaseFile,
                    subject,
                    from.display().to_string(),
                    to.display().to_string(),
                    PredecessorOutcome::Failed(error),
                );
                return false;
            }
        }
    }
    true
}

/// Rename the memory ledger file to its canonical name, in the same
/// directory, without decoding or rewriting it and without a health check.
/// The committed bytes must still be the planned ones and the canonical
/// name still free. That is a check, not an exclusion: nothing fences the
/// ledger's writer, so a failure here is reported, never resolved by
/// overwriting either file.
fn apply_memory_ledger_move(plan: &StatePlan, report: &mut PredecessorAdoptReport) -> bool {
    let Some(planned) = &plan.memory_ledger_move else {
        return true;
    };
    let moved = revalidate_ledger_move(planned)
        .and_then(|()| {
            std::fs::rename(&planned.from, &planned.to).map_err(|error| {
                format!(
                    "rename {} -> {}: {error}",
                    planned.from.display(),
                    planned.to.display()
                )
            })
        })
        // The journal records the step finished only once the rename is
        // durable.
        .and_then(|()| sync_parent(&planned.to));
    let outcome = match moved {
        Ok(()) => PredecessorOutcome::Applied,
        Err(error) => PredecessorOutcome::Failed(error),
    };
    let failed = matches!(outcome, PredecessorOutcome::Failed(_));
    record_memory_ledger(planned, outcome, report);
    !failed
}

/// The planned ledger rename still holds: the committed bytes are the ones
/// planned, no interrupted write left a stage beside them, and the canonical
/// name and its stage are still free.
fn revalidate_ledger_move(planned: &LedgerFileMove) -> Result<(), String> {
    if ledger_digest(&planned.from)? != planned.digest {
        return Err(format!(
            "{} changed since planning; another process is writing it",
            planned.from.display()
        ));
    }
    let stage = crate::runtime::staged_ledger_path(&planned.from);
    if present(&stage) {
        return Err(format!(
            "{} appeared since planning; another process is writing the ledger",
            stage.display()
        ));
    }
    if let Some(occupied) = ledger_names(&planned.to).find(|path| present(path)) {
        return Err(format!(
            "{} appeared since planning; refusing to write one history over another",
            occupied.display()
        ));
    }
    Ok(())
}

/// Rename the creator field of every planned composition manifest in place.
fn apply_composition_manifests(plan: &StatePlan, report: &mut PredecessorAdoptReport) -> bool {
    for planned in &plan.composition_manifests {
        // Under adoption every error is a failure: the run has already
        // changed state (its journal), whatever this rename did.
        let outcome = match rename_manifest_creator(planned) {
            Ok(()) => PredecessorOutcome::Applied,
            Err(error) => PredecessorOutcome::Failed(error.to_string()),
        };
        let failed = matches!(outcome, PredecessorOutcome::Failed(_));
        record_composition_manifest(planned, outcome, report);
        if failed {
            return false;
        }
    }
    true
}

/// Why one composition manifest rename did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestRepairError {
    /// Refused before anything changed: the manifest and its directory are
    /// as they were.
    Refused(String),
    /// Failed after something changed. Either the renamed manifest replaced
    /// the original but the replacement is not yet durable (a crash before
    /// the directory is synced may bring the original back), or this rename's
    /// own stage could not be removed again. Inspect before rerunning.
    Failed(String),
}

impl ManifestRepairError {
    /// How a report names this outcome: `refused` or `failed`.
    pub const fn outcome(&self) -> &'static str {
        match self {
            Self::Refused(_) => "refused",
            Self::Failed(_) => "failed",
        }
    }

    /// The reason, for a human.
    pub fn detail(&self) -> &str {
        match self {
            Self::Refused(detail) | Self::Failed(detail) => detail,
        }
    }
}

impl std::fmt::Display for ManifestRepairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.detail())
    }
}

impl std::error::Error for ManifestRepairError {}

/// Rename one planned manifest's creator field in place, keeping every other
/// byte. The bytes must still be the planned ones (a check, not an
/// exclusion: the stopped installation keeps writers away), and the
/// replacement is atomic: an owner-only stage written beside the manifest,
/// given the manifest's permissions, synced, renamed over it, and the
/// directory synced.
///
/// A failure before the replacement is [`ManifestRepairError::Refused`]: the
/// manifest is as it was, and this rename's own stage is removed again. A
/// failure after it (the directory sync), or a stage that cannot be removed,
/// is [`ManifestRepairError::Failed`].
fn rename_manifest_creator(planned: &ManifestRename) -> Result<(), ManifestRepairError> {
    rename_manifest_creator_with(planned, sync_parent)
}

/// [`rename_manifest_creator`] with the directory sync supplied, so a test
/// can fail it after the replacement.
fn rename_manifest_creator_with(
    planned: &ManifestRename,
    sync_directory: impl Fn(&Path) -> Result<(), String>,
) -> Result<(), ManifestRepairError> {
    use std::io::Write as _;
    let bytes = regular_file_bytes(&planned.path).map_err(ManifestRepairError::Refused)?;
    if <[u8; 32]>::from(Sha256::digest(&bytes)) != planned.digest {
        return Err(ManifestRepairError::Refused(format!(
            "{} changed since planning; another process is writing it",
            planned.path.display()
        )));
    }
    let ManifestCreator::Predecessor(key) =
        manifest_creator(&bytes).map_err(ManifestRepairError::Refused)?
    else {
        return Err(ManifestRepairError::Refused(format!(
            "{} no longer carries the predecessor creator field",
            planned.path.display()
        )));
    };
    let renamed = renamed_manifest(&bytes, key).map_err(ManifestRepairError::Refused)?;
    let permissions = std::fs::metadata(&planned.path)
        .map_err(|error| {
            ManifestRepairError::Refused(format!("inspect {}: {error}", planned.path.display()))
        })?
        .permissions();
    let stage = staged_manifest_path(&planned.path);
    let mut file = create_manifest_stage(&stage).map_err(|error| {
        ManifestRepairError::Refused(format!("create {}: {error}", stage.display()))
    })?;
    // From here the stage is ours: a failure removes it, and only it. Its
    // owner-only mode covers the bytes until the manifest's own permissions
    // replace it.
    let written = file
        .write_all(&renamed)
        .and_then(|()| file.set_permissions(permissions))
        .and_then(|()| file.sync_all());
    drop(file);
    let replaced = written
        .map_err(|error| format!("write {}: {error}", stage.display()))
        .and_then(|()| {
            std::fs::rename(&stage, &planned.path).map_err(|error| {
                format!(
                    "rename {} -> {}: {error}",
                    stage.display(),
                    planned.path.display()
                )
            })
        });
    if let Err(error) = replaced {
        return Err(match std::fs::remove_file(&stage) {
            Ok(()) => ManifestRepairError::Refused(error),
            Err(remove) if remove.kind() == std::io::ErrorKind::NotFound => {
                ManifestRepairError::Refused(error)
            }
            Err(remove) => ManifestRepairError::Failed(format!(
                "{error}; the stage {} could not be removed ({remove}) and is left behind",
                stage.display()
            )),
        });
    }
    // The manifest is replaced; only the replacement's durability is left.
    sync_directory(&planned.path).map_err(|error| {
        ManifestRepairError::Failed(format!(
            "{} now holds the renamed manifest, but the replacement is not yet durable: \
             {error} (a crash before its directory is synced may bring back the original; \
             a rerun reports which one is in place)",
            planned.path.display()
        ))
    })
}

/// Create a manifest rename's stage exclusively and, on Unix, owner-only, so
/// the manifest's bytes are never readable to others before its own
/// permissions are applied. Elsewhere the platform's default applies.
fn create_manifest_stage(stage: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(stage)
}

/// What [`repair_composition_manifest`] found, or did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestRepair {
    /// The creator field already has its current spelling.
    Current,
    /// The creator field still has its predecessor spelling (dry run).
    Predecessor,
    /// The creator field was renamed in place.
    Rewritten,
}

/// Inspect one composition manifest and, under [`MigrateMode::Apply`],
/// rename a predecessor creator field in place: the adoption step's rename
/// for one file, behind `meta_repair --composition-manifests`. Every other
/// byte is kept, and a manifest this cannot judge (not a regular file, not a
/// JSON object, no creator field or more than one, an interrupted rename's
/// stage beside it) is refused unchanged. A failure after the replacement is
/// [`ManifestRepairError::Failed`], never a refusal. Like adoption, it relies
/// on every process using the store being stopped.
pub fn repair_composition_manifest(
    path: &Path,
    mode: MigrateMode,
) -> Result<ManifestRepair, ManifestRepairError> {
    repair_composition_manifest_with(path, mode, sync_parent)
}

/// [`repair_composition_manifest`] with the directory sync supplied.
fn repair_composition_manifest_with(
    path: &Path,
    mode: MigrateMode,
    sync_directory: impl Fn(&Path) -> Result<(), String>,
) -> Result<ManifestRepair, ManifestRepairError> {
    let Some(planned) = plan_composition_manifest(path).map_err(ManifestRepairError::Refused)?
    else {
        return Ok(ManifestRepair::Current);
    };
    if mode != MigrateMode::Apply {
        return Ok(ManifestRepair::Predecessor);
    }
    rename_manifest_creator_with(&planned, sync_directory).map(|()| ManifestRepair::Rewritten)
}

fn apply_ledgers(plan: &StatePlan, report: &mut PredecessorAdoptReport) -> bool {
    for ledger in &plan.ledgers {
        let outcome = match meerkat_sqlite::open(
            &ledger.database,
            meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
        ) {
            Ok(conn) => match rewrite_ledger(conn, &ledger.renames, ledger.moves_table) {
                Ok(()) => PredecessorOutcome::Applied,
                Err(error) => PredecessorOutcome::Failed(format!(
                    "this database's transaction rolled back ({error}); earlier steps stay applied"
                )),
            },
            Err(error) => PredecessorOutcome::Failed(format!("open failed: {error}")),
        };
        let failed = matches!(outcome, PredecessorOutcome::Failed(_));
        record_ledger(ledger, outcome, report);
        if failed {
            return false;
        }
    }
    true
}

fn rewrite_ledger(
    mut conn: rusqlite::Connection,
    renames: &[(&str, &str)],
    moves_table: bool,
) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction()?;
    for (from, to) in renames {
        tx.execute(
            "UPDATE meerkat_schema SET domain = ?2 WHERE domain = ?1",
            rusqlite::params![from, to],
        )?;
    }
    if moves_table {
        // Recreate through the current DDL rather than `ALTER TABLE RENAME`,
        // so the catalog matches a fresh database byte for byte.
        crate::runtime::create_metadata_table(&tx)?;
        tx.execute_batch(&format!(
            "INSERT INTO {to} (mob_id, key, value) SELECT mob_id, key, value FROM {from};
             DROP TABLE {from};",
            to = crate::runtime::METADATA_TABLE,
            from = PREDECESSOR_METADATA_TABLE,
        ))?;
    }
    tx.commit()
}

fn read_ledger_domains(
    conn: &rusqlite::Connection,
) -> Result<Option<Vec<String>>, rusqlite::Error> {
    if !table_exists(conn, "meerkat_schema")? {
        return Ok(None);
    }
    let mut statement = conn.prepare("SELECT domain FROM meerkat_schema ORDER BY domain")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    rows.collect::<Result<Vec<_>, _>>().map(Some)
}

fn table_exists(conn: &rusqlite::Connection, table: &str) -> Result<bool, rusqlite::Error> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
}

/// Retarget every planned schedule to the renamed steward runnable through
/// the schedule service (revision-checked, with its normal occurrence
/// replanning). Runs only under custody, after the read-only plan.
async fn apply_schedules(
    state_dir: &Path,
    plan: &StatePlan,
    report: &mut PredecessorAdoptReport,
) -> bool {
    if plan.schedules.is_empty() {
        return true;
    }
    let store_path =
        MetaStorageLayout::with_injected_roots(state_dir.to_path_buf(), None).schedule_db();
    let subject = store_path.display().to_string();
    let service = match SqliteScheduleStore::open(&store_path) {
        Ok(store) => ScheduleService::new(std::sync::Arc::new(store)),
        Err(error) => {
            report.record(
                PredecessorItem::StewardSchedule,
                subject,
                "",
                "",
                PredecessorOutcome::Failed(format!("open failed: {error}")),
            );
            return false;
        }
    };
    for schedule_id in &plan.schedules {
        let outcome = match retarget_steward_schedule(&service, schedule_id).await {
            Ok(()) => PredecessorOutcome::Applied,
            Err(error) => PredecessorOutcome::Failed(error),
        };
        let failed = matches!(outcome, PredecessorOutcome::Failed(_));
        report.record(
            PredecessorItem::StewardSchedule,
            format!("{subject} schedule {schedule_id}"),
            PREDECESSOR_STEWARD_DREAM_RUNNABLE,
            crate::schedule_wiring::STEWARD_DREAM_RUNNABLE,
            outcome,
        );
        if failed {
            return false;
        }
    }
    true
}

async fn retarget_steward_schedule(
    service: &ScheduleService,
    schedule_id: &str,
) -> Result<(), String> {
    let schedule = service
        .list()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|schedule| schedule.schedule_id.to_string() == schedule_id)
        .ok_or_else(|| format!("schedule {schedule_id} is gone"))?;
    let TargetBinding::HostRunnable(binding) = &schedule.target else {
        return Err(format!(
            "schedule {schedule_id} no longer targets a host runnable"
        ));
    };
    if binding.runnable.as_str() != PREDECESSOR_STEWARD_DREAM_RUNNABLE {
        return Err(format!(
            "schedule {schedule_id} no longer targets the predecessor runnable"
        ));
    }
    let runnable = HostRunnableName::parse(crate::schedule_wiring::STEWARD_DREAM_RUNNABLE)
        .map_err(|error| error.to_string())?;
    let target = TargetBinding::host_runnable(HostRunnableTargetBinding {
        runnable,
        params: binding.params.clone(),
    });
    service
        .update(
            &schedule.schedule_id,
            UpdateScheduleRequest {
                expected_revision: Some(schedule.revision),
                name: Some(crate::schedule_wiring::STEWARD_DREAM_SCHEDULE_NAME.to_string()),
                target: Some(target),
                ..Default::default()
            },
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Move the predecessor gateway home, then prove the established identity
/// arrived intact: the same public key, and the same host pins and
/// audience bindings.
fn apply_home_move(plan: &HomePlan, report: &mut PredecessorAdoptReport) -> bool {
    let Some((predecessor, home)) = &plan.home_move else {
        return true;
    };
    let subject = predecessor.display().to_string();
    let moved = if home.is_dir() {
        // An empty renamed home (planning proved it empty): replace it.
        std::fs::remove_dir(home).and_then(|()| std::fs::rename(predecessor, home))
    } else {
        home.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::rename(predecessor, home))
    }
    .map_err(|error| error.to_string())
    // Both parents (usually one directory) hold the renamed entries.
    .and_then(|()| sync_parent(predecessor))
    .and_then(|()| sync_parent(home));
    if let Err(error) = moved {
        report.record(
            PredecessorItem::GatewayHome,
            subject,
            predecessor.display().to_string(),
            home.display().to_string(),
            PredecessorOutcome::Failed(format!("move failed: {error}")),
        );
        return false;
    }
    report.record(
        PredecessorItem::GatewayHome,
        subject,
        predecessor.display().to_string(),
        home.display().to_string(),
        PredecessorOutcome::Applied,
    );
    let outcome = match established_identity(home) {
        Ok(identity) if Some(&identity) == plan.identity.as_ref() => PredecessorOutcome::Applied,
        Ok(_) => PredecessorOutcome::Failed(
            "the moved home no longer carries the established key, pins or audiences".into(),
        ),
        Err(error) => PredecessorOutcome::Failed(error),
    };
    let failed = matches!(outcome, PredecessorOutcome::Failed(_));
    report.record(
        PredecessorItem::PeerIdentity,
        home.display().to_string(),
        predecessor.display().to_string(),
        home.display().to_string(),
        outcome,
    );
    !failed
}

fn apply_draft_rename(plan: &HomePlan, report: &mut PredecessorAdoptReport) -> bool {
    let Some((from, to)) = &plan.draft_rename else {
        return true;
    };
    let renamed = std::fs::rename(from, to)
        .map_err(|error| error.to_string())
        .and_then(|()| sync_parent(to));
    let outcome = match renamed {
        Ok(()) => PredecessorOutcome::Applied,
        Err(error) => PredecessorOutcome::Failed(format!("rename failed: {error}")),
    };
    let failed = matches!(outcome, PredecessorOutcome::Failed(_));
    report.record(
        PredecessorItem::DraftStore,
        from.display().to_string(),
        from.display().to_string(),
        to.display().to_string(),
        outcome,
    );
    !failed
}

fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    use crate::runtime::SqliteMetadataStore;
    use crate::runtime::remote_host::{HOST_PAIRING_FILE_NAME, HostPairingStore};

    /// A predecessor metadata database as the predecessor release wrote it:
    /// its ledger row and its table, one label row.
    fn write_predecessor_metadata_db(path: &Path) {
        let conn = rusqlite::Connection::open(path).expect("open predecessor fixture");
        conn.execute_batch(
            "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);
             INSERT INTO meerkat_schema (domain, version) VALUES ('mobkit-metadata', 1);
             CREATE TABLE IF NOT EXISTS mobkit_metadata (
                 mob_id TEXT NOT NULL,
                 key    TEXT NOT NULL,
                 value  TEXT NOT NULL,
                 PRIMARY KEY (mob_id, key)
             );
             INSERT INTO mobkit_metadata (mob_id, key, value) VALUES ('mob-a', 'label', 'kept');",
        )
        .expect("write predecessor fixture");
    }

    async fn write_predecessor_steward_schedule(state: &Path) {
        let store_path =
            MetaStorageLayout::with_injected_roots(state.to_path_buf(), None).schedule_db();
        let store = SqliteScheduleStore::open(&store_path).expect("schedule store");
        let service = ScheduleService::new(std::sync::Arc::new(store));
        service
            .create(meerkat::CreateScheduleRequest {
                name: Some("predecessor-steward-dream".to_string()),
                description: None,
                trigger: meerkat::TriggerSpec::Interval(meerkat::IntervalTriggerSpec {
                    start_at_utc: chrono::Utc::now() + chrono::Duration::hours(1),
                    every_seconds: 3600,
                    end_at_utc: None,
                }),
                target: TargetBinding::host_runnable(HostRunnableTargetBinding {
                    runnable: HostRunnableName::parse(PREDECESSOR_STEWARD_DREAM_RUNNABLE).unwrap(),
                    params: None,
                }),
                misfire_policy: meerkat::MisfirePolicy::default(),
                overlap_policy: meerkat::OverlapPolicy::default(),
                missing_target_policy: meerkat::MissingTargetPolicy::default(),
                labels: std::collections::BTreeMap::new(),
                planning_horizon_days: None,
                planning_horizon_occurrences: None,
            })
            .await
            .expect("predecessor steward schedule");
        drop(service);
        // The store's connection worker closes its connection (and
        // checkpoints its WAL) on its own thread after the store is dropped,
        // holding its operation guard until then. Taking the exclusive fence
        // waits for exactly that, so the fixture is quiescent before any
        // snapshot.
        drop(
            meerkat_sqlite::ExclusiveFence::acquire(
                &store_path,
                std::time::Duration::from_secs(30),
            )
            .expect("schedule store worker closed"),
        );
    }

    async fn steward_runnables(state: &Path) -> Vec<String> {
        let store_path =
            MetaStorageLayout::with_injected_roots(state.to_path_buf(), None).schedule_db();
        let store = SqliteScheduleStore::open(&store_path).expect("reopen schedule store");
        ScheduleService::new(std::sync::Arc::new(store))
            .list()
            .await
            .expect("list")
            .iter()
            .filter_map(|schedule| match &schedule.target {
                TargetBinding::HostRunnable(binding) => Some(binding.runnable.as_str().to_string()),
                _ => None,
            })
            .collect()
    }

    /// Every file under `dir` with its bytes, except the WAL sidecars a
    /// read-only reader of a WAL database may need to create: the `-shm`
    /// index, and an empty `-wal`. Logical content never changes.
    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.to_string_lossy().to_string();
            let bytes = if path.is_file() {
                std::fs::read(&path).unwrap()
            } else {
                continue;
            };
            if name.ends_with("-shm") || (name.ends_with("-wal") && bytes.is_empty()) {
                continue;
            }
            files.push((path, bytes));
        }
        files.sort();
        files
    }

    fn request(state: &Path, mode: MigrateMode) -> PredecessorAdoptRequest {
        PredecessorAdoptRequest {
            state_dir: state.to_path_buf(),
            mode,
            ..PredecessorAdoptRequest::default()
        }
    }

    /// A predecessor memory ledger as its writer left it, at its predecessor
    /// name in the state directory; returns its bytes. Adoption never
    /// decodes it: the bytes only have to survive the rename exactly.
    fn write_predecessor_ledger(state: &Path) -> Vec<u8> {
        let bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "assertions": [{
                "assertion_id": "memory-assert-000001",
                "entity": "delivery",
                "topic": "email_send",
                "store": "todo",
                "fact": "double-check recipient consent",
                "metadata": null,
                "indexed_at_ms": 1
            }],
            "conflicts": []
        }))
        .unwrap();
        std::fs::write(state.join(PREDECESSOR_MEMORY_LEDGER_FILE_NAME), &bytes).unwrap();
        bytes
    }

    fn predecessor_ledger(state: &Path) -> PathBuf {
        state.join(PREDECESSOR_MEMORY_LEDGER_FILE_NAME)
    }

    fn canonical_ledger(state: &Path) -> PathBuf {
        state.join(crate::storage_layout::MEMORY_LEDGER_FILE_NAME)
    }

    fn ledger_refused(report: &PredecessorAdoptReport) -> bool {
        report.actions.iter().any(|action| {
            action.item == PredecessorItem::MemoryLedger
                && matches!(action.outcome, PredecessorOutcome::Refused(_))
        })
    }

    /// A predecessor home with an established identity: a real 32-byte
    /// peer key, one pinned host with its control audience, and drafts.
    fn write_enrolled_home(home: &Path) -> (String, String) {
        let key = crate::GatewayPeerKeys::load_or_create(home).expect("real peer key");
        let host_key = crate::GatewayPeerKeys::ephemeral().pubkey_b64();
        let pairings = serde_json::json!({
            "hosts": {
                host_key.clone(): {
                    "endpoint": "tcp://127.0.0.1:7001",
                    "host_label": "host-a",
                    "paired_at_unix_secs": 1,
                    "pairing_generation": 2,
                    "control_audience": "audience-a",
                }
            }
        });
        std::fs::write(
            home.join(HOST_PAIRING_FILE_NAME),
            serde_json::to_vec_pretty(&pairings).unwrap(),
        )
        .unwrap();
        std::fs::write(home.join(PREDECESSOR_DRAFT_STORE_FILE_NAME), b"{}").unwrap();
        (key.pubkey_b64(), host_key)
    }

    fn assert_identity_preserved(home: &Path, pubkey: &str, host_key: &str) {
        assert_eq!(
            crate::GatewayPeerKeys::load_existing(home)
                .expect("established key loads")
                .pubkey_b64(),
            pubkey
        );
        let pairings =
            HostPairingStore::load(home.join(HOST_PAIRING_FILE_NAME)).expect("pins load");
        let record = pairings.get(host_key).unwrap().expect("pin kept");
        assert_eq!(record.control_audience.as_deref(), Some("audience-a"));
        assert_eq!(record.pairing_generation, 2);
    }

    /// The one shell word the predecessor refusal must print for `path`,
    /// spelled out here from the rule (bare when every character is
    /// shell-safe, otherwise single-quoted with an embedded ' spelled '\''),
    /// so an expectation covers the complete path wherever the temp
    /// directory lives, including a temp parent that itself needs quoting.
    fn expected_shell_word(path: &Path) -> String {
        let text = path.to_string_lossy();
        if text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_./:=@%+-".contains(ch))
        {
            text.into_owned()
        } else {
            format!("'{}'", text.replace('\'', "'\\''"))
        }
    }

    #[test]
    fn predecessor_files_are_detected_and_block_resolution() {
        let temp = tempfile::tempdir().unwrap();
        write_predecessor_metadata_db(&temp.path().join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        assert_eq!(predecessor_database_files(temp.path()).len(), 1);
        let layout = MetaStorageLayout::with_injected_roots(temp.path().to_path_buf(), None);
        let error = layout.metadata_db().expect_err("predecessor state refuses");
        assert!(matches!(
            error,
            crate::storage_layout::StorageLayoutError::PredecessorState { .. }
        ));
        // The instruction is bound to the directory this layout resolved,
        // never a placeholder the operator has to fill in.
        assert!(
            error.to_string().contains(&format!(
                "storage-adopt-predecessor --state-dir {} --apply",
                expected_shell_word(temp.path())
            )),
            "{error}"
        );

        // A directory whose path needs quoting still gets a command that works
        // when copied: the path is one shell word, single-quoted, with an
        // embedded single quote spelled '\'' and nothing expanded.
        let parent = tempfile::tempdir().unwrap();
        let spaced = parent.path().join("state dir 'q' $HOME;x");
        std::fs::create_dir(&spaced).unwrap();
        write_predecessor_metadata_db(&spaced.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        let message = MetaStorageLayout::with_injected_roots(spaced.clone(), None)
            .metadata_db()
            .expect_err("predecessor state refuses")
            .to_string();
        let quoted = expected_shell_word(&spaced);
        // The basename forces quoting whatever the temp parent is, so the
        // whole path is one single-quoted word ending in the escaped basename.
        assert!(
            quoted.starts_with('\'')
                && quoted.ends_with(&format!(
                    "{}state dir '\\''q'\\'' $HOME;x'",
                    std::path::MAIN_SEPARATOR
                )),
            "{quoted}"
        );
        assert!(
            message.contains(&format!(
                "storage-adopt-predecessor --state-dir {quoted} --apply"
            )),
            "{message}"
        );
        #[cfg(unix)]
        {
            let echoed = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("printf '%s' {quoted}"))
                .output()
                .expect("run sh");
            assert_eq!(
                String::from_utf8_lossy(&echoed.stdout),
                spaced.display().to_string(),
                "the quoted directory must reach the shell as the exact path"
            );
        }

        // The memory ledger file refuses on its own, and so does the stage
        // an interrupted write of it left behind. Detection does not depend
        // on whether this start enables memory, and changes nothing.
        for stage in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = if stage {
                crate::runtime::staged_ledger_path(&predecessor_ledger(temp.path()))
            } else {
                predecessor_ledger(temp.path())
            };
            std::fs::write(&path, b"{\"assertions\": [").unwrap();
            assert!(predecessor_database_files(temp.path()).is_empty());
            assert_eq!(predecessor_state_files(temp.path()), vec![path.clone()]);
            let error = MetaStorageLayout::with_injected_roots(temp.path().to_path_buf(), None)
                .metadata_db()
                .expect_err("a predecessor ledger refuses");
            assert!(
                matches!(
                    error,
                    crate::storage_layout::StorageLayoutError::PredecessorState { ref paths, .. }
                        if paths == &vec![path.clone()]
                ),
                "{error}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"{\"assertions\": [");
            assert!(!canonical_ledger(temp.path()).exists());
        }
    }

    #[test]
    fn unadopted_predecessor_table_refuses_a_direct_open() {
        // An embedder that opens the metadata store by path, bypassing the
        // layout, still cannot start an empty renamed table beside the
        // predecessor one.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("labels.sqlite3");
        write_predecessor_metadata_db(&path);
        assert!(SqliteMetadataStore::open(&path).is_err());
    }

    #[tokio::test]
    async fn dry_run_is_read_only_and_apply_carries_state_over_once() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        write_predecessor_steward_schedule(state).await;
        let ledger = write_predecessor_ledger(state);
        let before = snapshot(state);

        let dry = adopt_predecessor_state(&request(state, MigrateMode::DryRun)).await;
        assert!(!dry.has_errors(), "{:?}", dry.errors);
        assert!(!dry.changed_state);
        assert!(
            dry.actions
                .iter()
                .all(|action| action.outcome == PredecessorOutcome::Planned)
        );
        for item in [
            PredecessorItem::DatabaseFile,
            PredecessorItem::MemoryLedger,
            PredecessorItem::LedgerDomain,
            PredecessorItem::MetadataTable,
            PredecessorItem::StewardSchedule,
        ] {
            assert!(
                dry.actions.iter().any(|a| a.item == item),
                "{item:?}: {dry:?}"
            );
        }
        assert_eq!(
            snapshot(state),
            before,
            "dry-run must leave every database byte, the memory ledger, the schedule store and \
             its schema unchanged"
        );

        let apply = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(!apply.has_errors(), "{:?}", apply.errors);
        assert!(apply.changed_state && apply.complete);
        assert!(predecessor_state_files(state).is_empty());
        assert_eq!(
            std::fs::read(canonical_ledger(state)).unwrap(),
            ledger,
            "the ledger is renamed byte for byte, never decoded or rewritten"
        );
        let canonical = state.join(CANONICAL_METADATA_DB_FILE_NAME);
        drop(SqliteMetadataStore::open(&canonical).expect("adopted store opens"));
        let conn = rusqlite::Connection::open(&canonical).expect("reopen adopted store");
        let value: String = conn
            .query_row(
                "SELECT value FROM meta_metadata WHERE mob_id = 'mob-a' AND key = 'label'",
                [],
                |row| row.get(0),
            )
            .expect("label carried over");
        assert_eq!(value, "kept");
        drop(conn);
        assert_eq!(
            steward_runnables(state).await,
            vec![crate::schedule_wiring::STEWARD_DREAM_RUNNABLE.to_string()],
            "the steward schedule is retargeted, not duplicated"
        );
        let journal = read_adoption_journal(state).unwrap().unwrap();
        assert_eq!(journal.status, AdoptionStatus::Complete);
        assert_eq!(
            journal.completed,
            BTreeSet::from(AdoptionStep::ALL),
            "the journal records every step, the memory ledger file's included"
        );
        let layout = MetaStorageLayout::with_injected_roots(state.to_path_buf(), None);
        layout.metadata_db().expect("adopted installation resolves");

        let again = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(!again.has_errors(), "{:?}", again.errors);
        assert!(again.actions.is_empty(), "{:?}", again.actions);
        assert!(again.complete && !again.changed_state);
        assert_eq!(std::fs::read(canonical_ledger(state)).unwrap(), ledger);
    }

    fn steward_create_request() -> meerkat::CreateScheduleRequest {
        meerkat::CreateScheduleRequest {
            name: Some("predecessor-steward-dream".to_string()),
            description: None,
            trigger: meerkat::TriggerSpec::Interval(meerkat::IntervalTriggerSpec {
                start_at_utc: chrono::Utc::now() + chrono::Duration::hours(1),
                every_seconds: 3600,
                end_at_utc: None,
            }),
            target: TargetBinding::host_runnable(HostRunnableTargetBinding {
                runnable: HostRunnableName::parse(PREDECESSOR_STEWARD_DREAM_RUNNABLE).unwrap(),
                params: None,
            }),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: std::collections::BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        }
    }

    /// A carried production-shaped schedule store: one live steward
    /// schedule whose `schedule_json` is stored as TEXT, and one deleted
    /// steward schedule. Returns (live id, deleted id).
    async fn write_carried_steward_schedules(state: &Path) -> (String, String) {
        let store_path =
            MetaStorageLayout::with_injected_roots(state.to_path_buf(), None).schedule_db();
        let store = SqliteScheduleStore::open(&store_path).expect("schedule store");
        let service = ScheduleService::new(std::sync::Arc::new(store));
        let live = service
            .create(steward_create_request())
            .await
            .expect("live steward schedule");
        let deleted = service
            .create(steward_create_request())
            .await
            .expect("second steward schedule");
        service
            .delete(&deleted.schedule_id)
            .await
            .expect("delete the second steward schedule");
        drop(service);
        drop(
            meerkat_sqlite::ExclusiveFence::acquire(
                &store_path,
                std::time::Duration::from_secs(30),
            )
            .expect("schedule store worker closed"),
        );
        let live_id = live.schedule_id.to_string();
        let conn = rusqlite::Connection::open(&store_path).expect("open schedule store");
        conn.execute(
            "UPDATE schedule_schedules SET schedule_json = CAST(schedule_json AS TEXT) \
             WHERE schedule_id = ?1",
            [&live_id],
        )
        .expect("store the live schedule's JSON as TEXT");
        let kind: String = conn
            .query_row(
                "SELECT typeof(schedule_json) FROM schedule_schedules WHERE schedule_id = ?1",
                [&live_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kind, "text");
        (live_id, deleted.schedule_id.to_string())
    }

    fn raw_schedule_json(state: &Path, schedule_id: &str) -> serde_json::Value {
        let store_path =
            MetaStorageLayout::with_injected_roots(state.to_path_buf(), None).schedule_db();
        let conn = rusqlite::Connection::open(&store_path).expect("open schedule store");
        let bytes = conn
            .query_row(
                "SELECT schedule_json FROM schedule_schedules WHERE schedule_id = ?1",
                [schedule_id],
                |row| row.get::<_, meerkat_sqlite::JsonColumnBytes>(0),
            )
            .unwrap()
            .into_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// The production shape: a TEXT `schedule_json` row and a deleted
    /// steward schedule. The plan names only the live schedule, and the
    /// apply retargets it to completion; the deleted one is left as it is.
    #[tokio::test]
    async fn a_text_row_and_a_deleted_steward_schedule_adopt_to_completion() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        let (live, deleted) = write_carried_steward_schedules(state).await;

        let dry = adopt_predecessor_state(&request(state, MigrateMode::DryRun)).await;
        assert!(!dry.has_errors(), "{:?}", dry.errors);
        let planned: Vec<&str> = dry
            .actions
            .iter()
            .filter(|action| action.item == PredecessorItem::StewardSchedule)
            .map(|action| action.subject.as_str())
            .collect();
        assert_eq!(planned.len(), 1, "{planned:?}");
        assert!(planned[0].ends_with(&live), "{planned:?}");

        let apply = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(!apply.has_errors(), "{:?}", apply.errors);
        assert!(apply.complete);
        assert_eq!(
            steward_runnables(state).await,
            vec![crate::schedule_wiring::STEWARD_DREAM_RUNNABLE.to_string()]
        );
        assert_eq!(
            raw_schedule_json(state, &deleted)["target"]["runnable"],
            PREDECESSOR_STEWARD_DREAM_RUNNABLE,
            "a deleted schedule is retired, not retargeted"
        );
        assert_eq!(
            read_adoption_journal(state).unwrap().unwrap().status,
            AdoptionStatus::Complete
        );
    }

    /// A planned action that could not complete is refused in the preflight,
    /// before the first change: no journal, no moved file, no marker.
    #[tokio::test]
    async fn an_unappliable_steward_schedule_is_refused_before_any_change() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        let (live, _) = write_carried_steward_schedules(state).await;
        let mut broken = raw_schedule_json(state, &live);
        broken["revision"] = serde_json::json!("not-a-revision");
        let store_path =
            MetaStorageLayout::with_injected_roots(state.to_path_buf(), None).schedule_db();
        rusqlite::Connection::open(&store_path)
            .unwrap()
            .execute(
                "UPDATE schedule_schedules SET schedule_json = ?1 WHERE schedule_id = ?2",
                rusqlite::params![serde_json::to_vec(&broken).unwrap(), live],
            )
            .unwrap();
        let before = snapshot(state);

        let apply = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(apply.has_errors());
        assert!(
            apply.actions.iter().any(|action| {
                action.item == PredecessorItem::StewardSchedule
                    && matches!(action.outcome, PredecessorOutcome::Refused(_))
            }),
            "{:?}",
            apply.actions
        );
        assert!(!apply.changed_state);
        assert!(read_adoption_journal(state).unwrap().is_none());
        assert_eq!(snapshot(state), before, "nothing changed");
    }

    /// A row whose JSON names a different schedule than its row id is
    /// refused in the preflight, before any change.
    #[tokio::test]
    async fn a_schedule_row_naming_another_schedule_is_refused_before_any_change() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        let (live, deleted) = write_carried_steward_schedules(state).await;
        let mut swapped = raw_schedule_json(state, &live);
        swapped["schedule_id"] = serde_json::json!(deleted);
        let store_path =
            MetaStorageLayout::with_injected_roots(state.to_path_buf(), None).schedule_db();
        rusqlite::Connection::open(&store_path)
            .unwrap()
            .execute(
                "UPDATE schedule_schedules SET schedule_json = ?1 WHERE schedule_id = ?2",
                rusqlite::params![serde_json::to_vec(&swapped).unwrap(), live],
            )
            .unwrap();
        let before = snapshot(state);

        let apply = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(apply.has_errors());
        assert!(!apply.changed_state);
        assert!(read_adoption_journal(state).unwrap().is_none());
        assert_eq!(snapshot(state), before, "nothing changed");
    }

    /// A carried production schedule store copied into a fresh state
    /// directory (never opened in place).
    fn carried_schedule_store(state: &Path, fixture: &str) {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/predecessor_adoption_steward")
            .join(fixture);
        let store_path =
            MetaStorageLayout::with_injected_roots(state.to_path_buf(), None).schedule_db();
        std::fs::copy(&source, &store_path).expect("copy the carried schedule store");
    }

    const CARRIED_LIVE_STEWARD: &str = "01a067bd";
    const CARRIED_DELETED_STEWARD: &str = "019fc856";

    /// Production reproducer 1: a carried store with one TEXT
    /// `schedule_json` row. The plan used to refuse at the SQL read
    /// ("Invalid column type Text"); it now reads the row. The packet's
    /// TEXT row has its target binding key redacted, so its typed decode
    /// fails its machine-state projection check and the preflight refuses it,
    /// before any change and for that reason, not the column type. (A valid
    /// TEXT row plans and adopts: see
    /// `a_text_row_and_a_deleted_steward_schedule_adopt_to_completion`.)
    #[tokio::test]
    async fn the_carried_text_row_is_read_and_typed_checked() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        carried_schedule_store(state, "text_row_schedule.sqlite");
        let before = snapshot(state);
        let dry = adopt_predecessor_state(&request(state, MigrateMode::DryRun)).await;
        let errors = dry.errors.join("\n");
        assert!(!errors.contains("Invalid column type"), "{errors}");
        assert!(
            errors.contains("2061e9dc") && errors.contains("is not a valid schedule"),
            "the redacted TEXT row is read, then refused by its typed decode: {errors}"
        );
        assert_eq!(snapshot(state), before, "a dry run changes nothing");
    }

    /// The TEXT positive control: a carried store whose self-consistent
    /// TEXT row (and a live steward schedule) adopts as it is, to Complete.
    #[tokio::test]
    async fn the_carried_synthetic_text_row_store_adopts_to_completion() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        carried_schedule_store(state, "text_row_synthetic_schedule.sqlite");
        let dry = adopt_predecessor_state(&request(state, MigrateMode::DryRun)).await;
        assert!(!dry.has_errors(), "{:?}", dry.errors);
        let apply = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(!apply.has_errors(), "{:?}", apply.errors);
        assert!(apply.complete);
        assert_eq!(
            read_adoption_journal(state).unwrap().unwrap().status,
            AdoptionStatus::Complete
        );
        let runnables = steward_runnables(state).await;
        assert!(
            runnables.contains(&crate::schedule_wiring::STEWARD_DREAM_RUNNABLE.to_string())
                && !runnables.contains(&PREDECESSOR_STEWARD_DREAM_RUNNABLE.to_string()),
            "{runnables:?}"
        );
    }

    /// Production reproducer 2: a carried store with a deleted and a live
    /// steward schedule. The apply used to fail after changes ("schedule
    /// ... is gone") and stay half-applied; it now completes, retargeting
    /// only the live schedule.
    #[tokio::test]
    async fn the_carried_deleted_steward_store_adopts_to_completion() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        carried_schedule_store(state, "deleted_steward_schedule.sqlite");
        let apply = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(!apply.has_errors(), "{:?}", apply.errors);
        assert!(apply.complete);
        assert_eq!(
            steward_runnables(state).await,
            vec![crate::schedule_wiring::STEWARD_DREAM_RUNNABLE.to_string()]
        );
        let store_path =
            MetaStorageLayout::with_injected_roots(state.to_path_buf(), None).schedule_db();
        let conn = rusqlite::Connection::open(&store_path).unwrap();
        let deleted_id: String = conn
            .query_row(
                "SELECT schedule_id FROM schedule_schedules WHERE schedule_id LIKE ?1",
                [format!("{CARRIED_DELETED_STEWARD}%")],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);
        assert_eq!(
            raw_schedule_json(state, &deleted_id)["target"]["runnable"],
            PREDECESSOR_STEWARD_DREAM_RUNNABLE,
            "the deleted schedule is retired, not retargeted"
        );
        assert_eq!(
            read_adoption_journal(state).unwrap().unwrap().status,
            AdoptionStatus::Complete
        );
    }

    /// A valid-JSON but typed-invalid UNRELATED row is refused in the
    /// preflight too: the apply's strict schedule listing would fail on it
    /// after changes.
    #[tokio::test]
    async fn an_unrelated_typed_invalid_row_is_refused_before_any_change() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        carried_schedule_store(state, "deleted_steward_schedule.sqlite");
        let store_path =
            MetaStorageLayout::with_injected_roots(state.to_path_buf(), None).schedule_db();
        let conn = rusqlite::Connection::open(&store_path).unwrap();
        let live_id: String = conn
            .query_row(
                "SELECT schedule_id FROM schedule_schedules WHERE schedule_id LIKE ?1",
                [format!("{CARRIED_LIVE_STEWARD}%")],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);
        // An unrelated (non-steward) row: the live schedule's columns under a
        // fresh id, with a JSON body the typed schedule rejects.
        let mut unrelated = raw_schedule_json(state, &live_id);
        let unrelated_id = "01a067bd-0000-7000-8000-000000000001";
        unrelated["schedule_id"] = serde_json::json!(unrelated_id);
        unrelated["target"] = serde_json::json!({"target_kind": "unknown_kind"});
        let conn = rusqlite::Connection::open(&store_path).unwrap();
        let columns: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('schedule_schedules')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let select: Vec<String> = columns
            .iter()
            .map(|column| match column.as_str() {
                "schedule_id" => "?1".to_string(),
                "schedule_json" => "?2".to_string(),
                other => other.to_string(),
            })
            .collect();
        conn.execute(
            &format!(
                "INSERT INTO schedule_schedules ({}) SELECT {} FROM schedule_schedules \
                 WHERE schedule_id = ?3",
                columns.join(", "),
                select.join(", ")
            ),
            rusqlite::params![
                unrelated_id,
                serde_json::to_vec(&unrelated).unwrap(),
                live_id
            ],
        )
        .unwrap();
        drop(conn);
        let before = snapshot(state);

        let apply = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(apply.has_errors());
        assert!(!apply.changed_state);
        assert!(read_adoption_journal(state).unwrap().is_none());
        assert_eq!(snapshot(state), before, "nothing changed");
    }

    #[tokio::test]
    async fn adoption_refuses_a_renamed_twin_without_changing_anything() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        drop(
            SqliteMetadataStore::open(state.join(CANONICAL_METADATA_DB_FILE_NAME))
                .expect("renamed twin"),
        );
        let report = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(report.has_errors());
        assert!(!report.changed_state);
        assert!(state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]).is_file());
        assert!(read_adoption_journal(state).unwrap().is_none());

        // The memory ledger: a renamed ledger, or the stage of a write to
        // one, beside the predecessor file refuses before any change. One
        // history is never merged into or written over another.
        for stage in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let state = temp.path();
            write_predecessor_ledger(state);
            let occupied = if stage {
                crate::runtime::staged_ledger_path(&canonical_ledger(state))
            } else {
                canonical_ledger(state)
            };
            std::fs::write(&occupied, b"another history").unwrap();
            let before = snapshot(state);
            for mode in [MigrateMode::DryRun, MigrateMode::Apply] {
                let report = adopt_predecessor_state(&request(state, mode)).await;
                assert!(report.has_errors(), "{report:?}");
                assert!(!report.changed_state);
                assert!(ledger_refused(&report), "{:?}", report.actions);
            }
            assert_eq!(snapshot(state), before, "nothing changed");
            assert!(read_adoption_journal(state).unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn a_concurrent_destination_holder_refuses_before_any_change() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        let predecessor = state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]);
        write_predecessor_metadata_db(&predecessor);
        let before = std::fs::read(&predecessor).unwrap();

        // A FOREIGN process opening the renamed destination holds its shared
        // operation lock.
        let destination = state.join(CANONICAL_METADATA_DB_FILE_NAME);
        let foreign = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(meerkat_sqlite::fence_lock_path(&destination))
            .expect("open foreign lock");
        foreign.try_lock_shared().expect("foreign shared lock");

        let report = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(report.has_errors());
        assert!(!report.changed_state, "{report:?}");
        assert!(
            report
                .actions
                .iter()
                .all(|action| matches!(action.outcome, PredecessorOutcome::Refused(_)))
        );
        assert_eq!(std::fs::read(&predecessor).unwrap(), before);
        assert!(!destination.exists());
        assert!(read_adoption_journal(state).unwrap().is_none());
        drop(foreign);
    }

    #[tokio::test]
    async fn an_interrupted_schedule_conversion_refuses_startup_until_resumed() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_steward_schedule(state).await;
        // The run that changed state stopped before the schedule step: the
        // journal is the only evidence (no predecessor file names remain).
        write_adoption_journal(
            state,
            &AdoptionJournal {
                version: ADOPTION_JOURNAL_VERSION,
                status: AdoptionStatus::InProgress,
                predecessor_home: None,
                gateway_home: None,
                completed: [AdoptionStep::DatabaseFiles, AdoptionStep::Ledger].into(),
                realm_id: None,
            },
        )
        .unwrap();
        assert!(predecessor_database_files(state).is_empty());
        let layout = MetaStorageLayout::with_injected_roots(state.to_path_buf(), None);
        let error = layout
            .metadata_db()
            .expect_err("unfinished adoption refuses startup");
        assert!(
            matches!(
                error,
                crate::storage_layout::StorageLayoutError::AdoptionIncomplete { .. }
            ),
            "{error}"
        );

        let resumed = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(!resumed.has_errors(), "{:?}", resumed.errors);
        assert!(resumed.complete);
        assert_eq!(
            steward_runnables(state).await,
            vec![crate::schedule_wiring::STEWARD_DREAM_RUNNABLE.to_string()]
        );
        // A layout resolves its installation once, at construction: the
        // layout built mid-adoption keeps refusing; a new one resolves.
        assert!(layout.metadata_db().is_err());
        MetaStorageLayout::with_injected_roots(state.to_path_buf(), None)
            .metadata_db()
            .expect("finished adoption resolves");
    }

    /// An adoption that completed before the memory ledger step existed
    /// does not hide a predecessor ledger: startup refuses, the dry run
    /// plans the rename and reports the adoption incomplete, and apply
    /// carries the file over instead of taking the already-complete
    /// shortcut.
    #[tokio::test]
    async fn a_ledger_found_after_a_completed_adoption_is_adopted_not_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_adoption_journal(
            state,
            &AdoptionJournal {
                version: ADOPTION_JOURNAL_VERSION,
                status: AdoptionStatus::Complete,
                predecessor_home: None,
                gateway_home: None,
                completed: [
                    AdoptionStep::DatabaseFiles,
                    AdoptionStep::Ledger,
                    AdoptionStep::StewardSchedule,
                    AdoptionStep::GatewayHome,
                    AdoptionStep::DraftStore,
                ]
                .into(),
                realm_id: None,
            },
        )
        .unwrap();
        let ledger = write_predecessor_ledger(state);

        let error = MetaStorageLayout::with_injected_roots(state.to_path_buf(), None)
            .metadata_db()
            .expect_err("a complete journal does not hide the predecessor ledger");
        assert!(
            matches!(
                error,
                crate::storage_layout::StorageLayoutError::PredecessorState { .. }
            ),
            "{error}"
        );

        let dry = adopt_predecessor_state(&request(state, MigrateMode::DryRun)).await;
        assert!(!dry.has_errors(), "{:?}", dry.errors);
        assert!(!dry.complete, "a planned rename is not a complete adoption");
        assert!(
            dry.actions.iter().any(|action| {
                action.item == PredecessorItem::MemoryLedger
                    && action.outcome == PredecessorOutcome::Planned
            }),
            "{:?}",
            dry.actions
        );

        let apply = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(!apply.has_errors(), "{:?}", apply.errors);
        assert!(apply.changed_state && apply.complete, "{apply:?}");
        assert!(
            apply.actions.iter().any(|action| {
                action.item == PredecessorItem::MemoryLedger
                    && action.outcome == PredecessorOutcome::Applied
            }),
            "{:?}",
            apply.actions
        );
        assert_eq!(std::fs::read(canonical_ledger(state)).unwrap(), ledger);
        assert!(!predecessor_ledger(state).exists());
        let journal = read_adoption_journal(state).unwrap().unwrap();
        assert_eq!(journal.status, AdoptionStatus::Complete);
        assert!(journal.completed.contains(&AdoptionStep::MemoryLedgerFile));
        MetaStorageLayout::with_injected_roots(state.to_path_buf(), None)
            .metadata_db()
            .expect("the adopted installation resolves");
    }

    /// The extension document store as a pre-0.9 writer left it. Its file
    /// name and schema are unchanged in 0.9 (the DDL below is the predecessor
    /// writer's, byte for byte); only its ledger domain was renamed. One
    /// document row.
    fn write_predecessor_extension_store(path: &Path) {
        let conn = rusqlite::Connection::open(path).expect("open predecessor fixture");
        conn.execute_batch(
            "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);
             INSERT INTO meerkat_schema (domain, version) VALUES ('mobkit-extension-state', 1);
CREATE TABLE IF NOT EXISTS extension_documents (
 realm TEXT NOT NULL, namespace TEXT NOT NULL, id TEXT NOT NULL,
 metadata TEXT NOT NULL, payload BLOB NOT NULL, deleted INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(realm, namespace, id));
CREATE TABLE IF NOT EXISTS extension_receipts (
 realm TEXT NOT NULL, namespace TEXT NOT NULL, principal TEXT NOT NULL,
 request_id TEXT NOT NULL, fingerprint BLOB NOT NULL, receipt TEXT NOT NULL,
 document_id TEXT NOT NULL, operation TEXT NOT NULL,
 PRIMARY KEY(realm, namespace, principal, request_id));
             INSERT INTO extension_documents (realm, namespace, id, metadata, payload)
                 VALUES ('meta', 'notes', 'kept-document', '{}', X'010203');",
        )
        .expect("write predecessor fixture");
    }

    // The public domain table keeps the type 0.9.1 released it with: the
    // extension store's domain is renamed beside it, not added to it.
    const _: [(&str, &str); 4] = PREDECESSOR_LEDGER_DOMAINS;

    fn ledger_domains_of(path: &Path) -> Vec<String> {
        let conn = rusqlite::Connection::open(path).expect("open the store");
        read_ledger_domains(&conn)
            .expect("read the ledger")
            .expect("the store has a ledger")
    }

    /// A pre-0.9 extension document store is predecessor state: startup
    /// refuses it typed and names the adoption, also in an installation whose
    /// adoption finished before this domain was adopted. The adoption renames
    /// its ledger domain under the fence and keeps every row, and the
    /// installation then resolves. Without the extension-state feature too:
    /// the stock gateway runs the adoption and fences the store.
    #[tokio::test]
    async fn an_extension_store_under_the_predecessor_domain_is_adopted_before_it_opens() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_adoption_journal(
            state,
            &AdoptionJournal {
                version: ADOPTION_JOURNAL_VERSION,
                status: AdoptionStatus::Complete,
                predecessor_home: None,
                gateway_home: None,
                completed: [
                    AdoptionStep::DatabaseFiles,
                    AdoptionStep::Ledger,
                    AdoptionStep::StewardSchedule,
                    AdoptionStep::GatewayHome,
                    AdoptionStep::DraftStore,
                ]
                .into(),
                realm_id: None,
            },
        )
        .unwrap();
        let store = state.join("extension_state.sqlite3");
        write_predecessor_extension_store(&store);

        let error =
            resolve_installation(state).expect_err("a predecessor extension store refuses startup");
        assert!(
            matches!(
                &error,
                crate::storage_layout::StorageLayoutError::PredecessorState { paths, .. }
                    if paths == std::slice::from_ref(&store)
            ),
            "{error}"
        );
        assert!(
            error.to_string().contains("storage-adopt-predecessor"),
            "{error}"
        );
        assert!(
            crate::storage_migrate::enumerate_state_dir_databases(state).contains(&store),
            "the maintenance fence covers the store"
        );

        let dry = adopt_predecessor_state(&request(state, MigrateMode::DryRun)).await;
        assert!(!dry.has_errors(), "{:?}", dry.errors);
        assert!(
            dry.actions.iter().any(|action| {
                action.item == PredecessorItem::LedgerDomain
                    && action.outcome == PredecessorOutcome::Planned
            }),
            "{:?}",
            dry.actions
        );
        assert_eq!(ledger_domains_of(&store), ["mobkit-extension-state"]);

        let apply = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(!apply.has_errors(), "{:?}", apply.errors);
        assert!(apply.changed_state && apply.complete, "{apply:?}");
        assert_eq!(ledger_domains_of(&store), ["meta-extension-state"]);
        let kept: i64 = rusqlite::Connection::open(&store)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM extension_documents WHERE id = 'kept-document'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kept, 1);
        resolve_installation(state).expect("the adopted installation resolves");
    }

    /// A rerun that reopens a finished adoption for predecessor state found
    /// since records none of the earlier run's steps as finished: when it
    /// fails, its journal and the unfinished-adoption refusal it carries
    /// name the steps this run still owes, not an empty list.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_reopened_adoption_that_fails_names_its_unfinished_steps() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_adoption_journal(
            state,
            &AdoptionJournal {
                version: ADOPTION_JOURNAL_VERSION,
                status: AdoptionStatus::Complete,
                predecessor_home: None,
                gateway_home: None,
                completed: AdoptionStep::ALL.into(),
                realm_id: None,
            },
        )
        .unwrap();
        let store = state.join("extension_state.sqlite3");
        write_predecessor_extension_store(&store);
        // The ledger step cannot write the store, so the reopened run fails
        // there, after the steps before it finished.
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o444)).unwrap();
        if std::fs::OpenOptions::new().write(true).open(&store).is_ok() {
            // A process that may write a read-only file (root) cannot fail
            // the step this way.
            return;
        }

        let apply = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(apply.has_errors(), "{apply:?}");
        assert!(!apply.complete);
        let journal = read_adoption_journal(state).unwrap().unwrap();
        assert_eq!(journal.status, AdoptionStatus::InProgress);
        assert_eq!(
            journal.completed,
            BTreeSet::from([AdoptionStep::DatabaseFiles, AdoptionStep::MemoryLedgerFile])
        );
        assert_eq!(
            unfinished_adoption(state).as_deref(),
            Some("steps not finished: Ledger, StewardSchedule, GatewayHome, DraftStore")
        );
        resolve_installation(state).expect_err("the unfinished adoption refuses startup");
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    /// The stage of an interrupted predecessor ledger write is evidence of
    /// the ledger's newest state. With or without the committed file beside
    /// it, it blocks startup and adoption refuses it for manual resolution:
    /// it is never deleted, promoted or renamed, and nothing else changes.
    #[tokio::test]
    async fn a_leftover_ledger_stage_is_preserved_and_refused() {
        for with_ledger in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let state = temp.path();
            if with_ledger {
                write_predecessor_ledger(state);
            }
            let stage = crate::runtime::staged_ledger_path(&predecessor_ledger(state));
            std::fs::write(&stage, b"{\"assertions\": [").unwrap();
            let before = snapshot(state);
            assert!(
                matches!(
                    resolve_installation(state),
                    Err(crate::storage_layout::StorageLayoutError::PredecessorState { .. })
                ),
                "with_ledger={with_ledger}"
            );

            for mode in [MigrateMode::DryRun, MigrateMode::Apply] {
                let report = adopt_predecessor_state(&request(state, mode)).await;
                assert!(report.has_errors(), "with_ledger={with_ledger}: {report:?}");
                assert!(!report.changed_state, "with_ledger={with_ledger}");
                assert!(ledger_refused(&report), "{:?}", report.actions);
                assert!(
                    report
                        .errors
                        .iter()
                        .any(|error| error.contains(&stage.display().to_string())),
                    "the refusal names the stage: {:?}",
                    report.errors
                );
            }
            assert_eq!(
                snapshot(state),
                before,
                "with_ledger={with_ledger}: every byte, the stage's included, is unchanged"
            );
            assert!(!canonical_ledger(state).exists());
            assert!(read_adoption_journal(state).unwrap().is_none());
        }
    }

    /// The run renamed the ledger and stopped before its journal recorded
    /// the step (a journal written before the step existed looks the same):
    /// startup refuses as unfinished, and a rerun completes from the
    /// canonical file with the same bytes.
    #[tokio::test]
    async fn an_interruption_after_the_ledger_rename_resumes_from_the_canonical_file() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        let ledger = write_predecessor_ledger(state);
        write_adoption_journal(
            state,
            &AdoptionJournal {
                version: ADOPTION_JOURNAL_VERSION,
                status: AdoptionStatus::InProgress,
                predecessor_home: None,
                gateway_home: None,
                completed: [AdoptionStep::DatabaseFiles].into(),
                realm_id: None,
            },
        )
        .unwrap();
        std::fs::rename(predecessor_ledger(state), canonical_ledger(state)).unwrap();

        let error = MetaStorageLayout::with_injected_roots(state.to_path_buf(), None)
            .metadata_db()
            .expect_err("an unfinished adoption refuses startup");
        assert!(
            matches!(
                error,
                crate::storage_layout::StorageLayoutError::AdoptionIncomplete { .. }
            ),
            "{error}"
        );
        assert!(error.to_string().contains("MemoryLedgerFile"), "{error}");

        let resumed = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(!resumed.has_errors(), "{:?}", resumed.errors);
        assert!(resumed.complete);
        assert_eq!(
            std::fs::read(canonical_ledger(state)).unwrap(),
            ledger,
            "the canonical file keeps the same bytes"
        );
        assert!(!predecessor_ledger(state).exists());
        let journal = read_adoption_journal(state).unwrap().unwrap();
        assert_eq!(journal.status, AdoptionStatus::Complete);
        assert!(journal.completed.contains(&AdoptionStep::MemoryLedgerFile));
        MetaStorageLayout::with_injected_roots(state.to_path_buf(), None)
            .metadata_db()
            .expect("finished adoption resolves");
    }

    /// The rename itself rechecks the plan: a destination taken since
    /// planning, or committed bytes changed since planning, fail the step
    /// without overwriting or moving either file, and a re-plan refuses or
    /// differs (so the fenced revalidation refuses before any change).
    #[test]
    fn the_ledger_rename_rechecks_its_bytes_and_destination_and_never_overwrites() {
        // The canonical name is taken after planning.
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        let ledger = write_predecessor_ledger(state);
        let mut report = PredecessorAdoptReport::default();
        let plan = plan_state(state, &mut report).expect("the ledger plans");
        assert!(plan.memory_ledger_move.is_some());
        std::fs::write(canonical_ledger(state), b"another history").unwrap();
        assert!(!apply_memory_ledger_move(&plan, &mut report));
        assert!(matches!(
            report.actions.last().map(|action| &action.outcome),
            Some(PredecessorOutcome::Failed(_))
        ));
        assert_eq!(
            std::fs::read(canonical_ledger(state)).unwrap(),
            b"another history"
        );
        assert_eq!(std::fs::read(predecessor_ledger(state)).unwrap(), ledger);
        let mut replan = PredecessorAdoptReport::default();
        assert!(plan_state(state, &mut replan).is_none());
        assert!(ledger_refused(&replan), "{:?}", replan.actions);

        // The committed bytes change after planning.
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_ledger(state);
        let mut report = PredecessorAdoptReport::default();
        let plan = plan_state(state, &mut report).expect("the ledger plans");
        std::fs::write(predecessor_ledger(state), b"{\"assertions\": []}").unwrap();
        assert!(!apply_memory_ledger_move(&plan, &mut report));
        assert!(!canonical_ledger(state).exists());
        assert_eq!(
            std::fs::read(predecessor_ledger(state)).unwrap(),
            b"{\"assertions\": []}"
        );
        let replanned = plan_state(state, &mut PredecessorAdoptReport::default());
        assert!(replanned.is_some_and(|replanned| replanned != plan));
    }

    #[tokio::test]
    async fn established_identity_moves_with_its_key_pins_and_audiences() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let predecessor = temp.path().join(PREDECESSOR_GATEWAY_HOME_DIR_NAME);
        let home = temp.path().join(GATEWAY_HOME_DIR_NAME);
        let (pubkey, host_key) = write_enrolled_home(&predecessor);
        assert_eq!(
            predecessor_gateway_home_beside(&home),
            Some(predecessor.clone())
        );
        assert!(matches!(
            crate::storage_layout::refuse_unadopted_predecessor_home(home.clone()),
            Err(crate::storage_layout::StorageLayoutError::PredecessorGatewayHome { .. })
        ));

        let report = adopt_predecessor_state(&PredecessorAdoptRequest {
            state_dir: state.clone(),
            predecessor_home: Some(predecessor.clone()),
            gateway_home: Some(home.clone()),
            homes_explicit: true,
            mode: MigrateMode::Apply,
        })
        .await;
        assert!(!report.has_errors(), "{:?}", report.errors);
        assert!(report.complete);
        assert!(!predecessor.exists());
        assert_identity_preserved(&home, &pubkey, &host_key);
        assert!(
            home.join(crate::mobpack::MOBPACK_DRAFT_STORE_FILENAME)
                .is_file()
        );
        assert_eq!(
            crate::storage_layout::refuse_unadopted_predecessor_home(home.clone()).unwrap(),
            home
        );
    }

    #[tokio::test]
    async fn an_interruption_between_home_and_drafts_resumes_safely() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let predecessor = temp.path().join(PREDECESSOR_GATEWAY_HOME_DIR_NAME);
        let home = temp.path().join(GATEWAY_HOME_DIR_NAME);
        let (pubkey, host_key) = write_enrolled_home(&predecessor);
        // The run stopped after the home moved and before the drafts were
        // renamed.
        std::fs::rename(&predecessor, &home).unwrap();
        write_adoption_journal(
            &state,
            &AdoptionJournal {
                version: ADOPTION_JOURNAL_VERSION,
                status: AdoptionStatus::InProgress,
                predecessor_home: Some(predecessor.clone()),
                gateway_home: Some(home.clone()),
                completed: [
                    AdoptionStep::DatabaseFiles,
                    AdoptionStep::Ledger,
                    AdoptionStep::StewardSchedule,
                    AdoptionStep::GatewayHome,
                ]
                .into(),
                realm_id: None,
            },
        )
        .unwrap();
        assert!(
            crate::storage_layout::refuse_unadopted_predecessor_home(home.clone()).is_err(),
            "a home still carrying the predecessor draft store refuses"
        );
        assert!(
            MetaStorageLayout::with_injected_roots(state.clone(), None)
                .metadata_db()
                .is_err()
        );

        let resumed = adopt_predecessor_state(&PredecessorAdoptRequest {
            state_dir: state.clone(),
            predecessor_home: Some(predecessor.clone()),
            gateway_home: Some(home.clone()),
            homes_explicit: true,
            mode: MigrateMode::Apply,
        })
        .await;
        assert!(!resumed.has_errors(), "{:?}", resumed.errors);
        assert!(resumed.complete);
        assert!(
            home.join(crate::mobpack::MOBPACK_DRAFT_STORE_FILENAME)
                .is_file()
        );
        assert!(!home.join(PREDECESSOR_DRAFT_STORE_FILE_NAME).exists());
        assert_identity_preserved(&home, &pubkey, &host_key);
        crate::storage_layout::refuse_unadopted_predecessor_home(home.clone()).unwrap();
        MetaStorageLayout::with_injected_roots(state, None)
            .metadata_db()
            .expect("finished adoption resolves");
    }

    #[tokio::test]
    async fn an_enrolled_home_without_a_usable_key_refuses_and_is_never_reminted() {
        for key_bytes in [None, Some(&b"key"[..])] {
            let temp = tempfile::tempdir().unwrap();
            let state = temp.path().join("state");
            std::fs::create_dir_all(&state).unwrap();
            let predecessor = temp.path().join(PREDECESSOR_GATEWAY_HOME_DIR_NAME);
            let home = temp.path().join(GATEWAY_HOME_DIR_NAME);
            write_enrolled_home(&predecessor);
            let key_path = predecessor.join(crate::auth::peer_keys::KEY_FILE_NAME);
            match key_bytes {
                None => std::fs::remove_file(&key_path).unwrap(),
                Some(bytes) => std::fs::write(&key_path, bytes).unwrap(),
            }

            let report = adopt_predecessor_state(&PredecessorAdoptRequest {
                state_dir: state.clone(),
                predecessor_home: Some(predecessor.clone()),
                gateway_home: Some(home.clone()),
                homes_explicit: true,
                mode: MigrateMode::Apply,
            })
            .await;
            assert!(report.has_errors());
            assert!(!report.changed_state);
            assert!(
                report
                    .actions
                    .iter()
                    .any(|a| a.item == PredecessorItem::PeerIdentity)
            );
            assert!(predecessor.is_dir() && !home.exists());

            // The runtime path refuses too: an enrolled installation never
            // gets a freshly minted identity in place of its own.
            assert!(crate::GatewayPeerKeys::load_or_create(&predecessor).is_err());
            match key_bytes {
                None => assert!(!key_path.exists()),
                Some(bytes) => assert_eq!(std::fs::read(&key_path).unwrap(), bytes),
            }
        }
    }

    #[tokio::test]
    async fn gateway_home_refuses_to_merge_two_identities() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let predecessor = temp.path().join(PREDECESSOR_GATEWAY_HOME_DIR_NAME);
        let home = temp.path().join(GATEWAY_HOME_DIR_NAME);
        let old = crate::GatewayPeerKeys::load_or_create(&predecessor)
            .unwrap()
            .pubkey_b64();
        let new = crate::GatewayPeerKeys::load_or_create(&home)
            .unwrap()
            .pubkey_b64();
        let report = adopt_predecessor_state(&PredecessorAdoptRequest {
            state_dir: state,
            predecessor_home: Some(predecessor.clone()),
            gateway_home: Some(home.clone()),
            homes_explicit: true,
            mode: MigrateMode::Apply,
        })
        .await;
        assert!(report.has_errors());
        assert!(!report.changed_state);
        assert_eq!(
            crate::GatewayPeerKeys::load_existing(&predecessor)
                .unwrap()
                .pubkey_b64(),
            old
        );
        assert_eq!(
            crate::GatewayPeerKeys::load_existing(&home)
                .unwrap()
                .pubkey_b64(),
            new
        );
    }

    fn journal_with_realm(state: &Path, realm_id: Option<&str>) {
        write_adoption_journal(
            state,
            &AdoptionJournal {
                version: ADOPTION_JOURNAL_VERSION,
                status: AdoptionStatus::Complete,
                predecessor_home: None,
                gateway_home: None,
                completed: AdoptionStep::ALL.into(),
                realm_id: realm_id.map(str::to_string),
            },
        )
        .unwrap();
    }

    #[test]
    fn a_fresh_installation_uses_the_meta_realm() {
        let temp = tempfile::tempdir().unwrap();
        let realm = resolve_installation(temp.path()).expect("fresh installation resolves");
        assert_eq!(
            realm.as_str(),
            crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID
        );
        let layout = MetaStorageLayout::with_injected_roots(temp.path().to_path_buf(), None);
        assert!(
            layout.jobs_db().unwrap().starts_with(
                temp.path()
                    .join(crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID)
            )
        );
    }

    #[tokio::test]
    async fn an_adopted_installation_keeps_its_recorded_predecessor_realm() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        let predecessor_jobs =
            meerkat_store::realm_paths_in(state, PREDECESSOR_REALM_ID).jobs_sqlite_path;
        std::fs::create_dir_all(predecessor_jobs.parent().unwrap()).unwrap();
        drop(meerkat::SqliteDetachedJobStore::open(&predecessor_jobs).expect("predecessor jobs"));

        let dry = adopt_predecessor_state(&request(state, MigrateMode::DryRun)).await;
        assert!(
            dry.actions
                .iter()
                .any(|a| a.item == PredecessorItem::RealmId
                    && a.outcome == PredecessorOutcome::Planned),
            "{:?}",
            dry.actions
        );
        let report = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(!report.has_errors(), "{:?}", report.errors);
        assert_eq!(
            read_adoption_journal(state)
                .unwrap()
                .unwrap()
                .realm_id
                .as_deref(),
            Some(PREDECESSOR_REALM_ID)
        );
        let layout = MetaStorageLayout::with_injected_roots(state.to_path_buf(), None);
        assert_eq!(
            layout.meerkat_level_realm().unwrap().as_str(),
            PREDECESSOR_REALM_ID
        );
        assert_eq!(layout.jobs_db().unwrap(), predecessor_jobs);
        assert!(
            !state
                .join(crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID)
                .exists()
        );
    }

    #[test]
    fn an_unadopted_predecessor_realm_directory_refuses_instead_of_orphaning_it() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(PREDECESSOR_REALM_ID)).unwrap();
        assert!(predecessor_database_files(temp.path()).is_empty());
        let error = resolve_installation(temp.path()).expect_err("unadopted realm refuses");
        assert!(
            matches!(
                error,
                crate::storage_layout::StorageLayoutError::PredecessorState { .. }
            ),
            "{error}"
        );
        let layout = MetaStorageLayout::with_injected_roots(temp.path().to_path_buf(), None);
        assert!(layout.jobs_db().is_err());
        assert!(layout.session_db().is_err());
    }

    #[tokio::test]
    async fn twin_realm_directories_refuse_typed() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        std::fs::create_dir_all(state.join(PREDECESSOR_REALM_ID)).unwrap();
        std::fs::create_dir_all(
            state.join(crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID),
        )
        .unwrap();
        journal_with_realm(state, Some(PREDECESSOR_REALM_ID));
        let error = resolve_installation(state).expect_err("twin realms refuse");
        assert!(
            matches!(
                error,
                crate::storage_layout::StorageLayoutError::RealmTwin { .. }
            ),
            "{error}"
        );
        std::fs::remove_file(state.join(ADOPTION_JOURNAL_FILE_NAME)).unwrap();
        let report = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(report.has_errors());
        assert!(!report.changed_state);
        assert!(
            report
                .actions
                .iter()
                .any(|a| a.item == PredecessorItem::RealmId)
        );
    }

    #[test]
    fn a_missing_or_invalid_recorded_realm_refuses_and_never_defaults() {
        use crate::storage_layout::StorageLayoutError;
        // Wrong value.
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("other")).unwrap();
        journal_with_realm(temp.path(), Some("other"));
        assert!(matches!(
            resolve_installation(temp.path()),
            Err(StorageLayoutError::AdoptedRealmInvalid { value: Some(ref value), .. }) if value == "other"
        ));
        // Recorded, but its realm directory is gone.
        let temp = tempfile::tempdir().unwrap();
        journal_with_realm(temp.path(), Some(PREDECESSOR_REALM_ID));
        assert!(matches!(
            resolve_installation(temp.path()),
            Err(StorageLayoutError::AdoptedRealmInvalid { .. })
        ));
        // Adopted, the predecessor realm directory exists, but nothing is
        // recorded.
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(PREDECESSOR_REALM_ID)).unwrap();
        journal_with_realm(temp.path(), None);
        assert!(matches!(
            resolve_installation(temp.path()),
            Err(StorageLayoutError::AdoptedRealmInvalid { value: None, .. })
        ));
        let layout = MetaStorageLayout::with_injected_roots(temp.path().to_path_buf(), None);
        assert!(layout.meerkat_level_realm().is_err());
        assert!(layout.jobs_db().is_err());
    }

    #[tokio::test]
    async fn a_torn_journal_refuses_typed_and_names_the_manual_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        journal_with_realm(state, None);
        let path = state.join(ADOPTION_JOURNAL_FILE_NAME);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();

        let error = resolve_installation(state).expect_err("a torn journal refuses");
        assert!(
            matches!(
                error,
                crate::storage_layout::StorageLayoutError::AdoptionJournalUnreadable { .. }
            ),
            "{error}"
        );
        let text = error.to_string();
        assert!(
            text.contains(".next") && text.contains("storage-adopt-predecessor"),
            "{text}"
        );
        assert!(
            MetaStorageLayout::with_injected_roots(state.to_path_buf(), None)
                .session_db()
                .is_err()
        );

        let report = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(report.has_errors());
        assert!(!report.changed_state);
        assert_eq!(std::fs::read(&path).unwrap(), &bytes[..bytes.len() / 2]);
    }

    #[test]
    fn the_environment_guard_matches_predecessor_variables_by_exact_name() {
        let names = [
            "EXAMPLE_MOBKIT_STATE_DIR",
            "EXAMPLE_MOBKIT_SHUTDOWN_SECS",
            "MOBKIT_MCP_FIXTURE_BIN",
            "MOBKIT_HTTP_LISTEN_ADDR",
            "META_HTTP_LISTEN_ADDR",
            "PATH",
        ]
        .map(str::to_string);
        assert_eq!(
            predecessor_variables_among(names),
            vec!["MOBKIT_HTTP_LISTEN_ADDR".to_string()]
        );
    }

    #[test]
    fn an_explicit_session_file_cannot_bypass_an_installation_refusal() {
        let session_file = |state: &Path| {
            MetaStorageLayout::standalone_from_store_path(
                &state.join("custom-sessions.sqlite3"),
                state.join("home"),
            )
            .session_db()
        };
        // Control: a fresh installation honours the explicit file.
        let fresh = tempfile::tempdir().unwrap();
        let resolved = session_file(fresh.path()).expect("valid override resolves");
        assert_eq!(
            resolved.provenance,
            crate::storage_layout::DatabaseProvenance::ExplicitOverride
        );
        // Unfinished adoption.
        let unfinished = tempfile::tempdir().unwrap();
        write_adoption_journal(
            unfinished.path(),
            &AdoptionJournal {
                version: ADOPTION_JOURNAL_VERSION,
                status: AdoptionStatus::InProgress,
                predecessor_home: None,
                gateway_home: None,
                completed: BTreeSet::new(),
                realm_id: None,
            },
        )
        .unwrap();
        assert!(matches!(
            session_file(unfinished.path()),
            Err(crate::storage_layout::StorageLayoutError::AdoptionIncomplete { .. })
        ));
        // Twin realms.
        let twin = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(twin.path().join(PREDECESSOR_REALM_ID)).unwrap();
        std::fs::create_dir_all(
            twin.path()
                .join(crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID),
        )
        .unwrap();
        assert!(matches!(
            session_file(twin.path()),
            Err(crate::storage_layout::StorageLayoutError::RealmTwin { .. })
        ));
        // Invalid recorded realm.
        let invalid = tempfile::tempdir().unwrap();
        journal_with_realm(invalid.path(), Some("other"));
        assert!(matches!(
            session_file(invalid.path()),
            Err(crate::storage_layout::StorageLayoutError::AdoptedRealmInvalid { .. })
        ));
        for state in [unfinished.path(), twin.path(), invalid.path()] {
            assert!(!state.join("custom-sessions.sqlite3").exists());
        }
    }

    #[tokio::test]
    async fn an_unfinished_home_adoption_resumes_only_with_its_recorded_homes() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let predecessor = temp
            .path()
            .join("custom")
            .join(PREDECESSOR_GATEWAY_HOME_DIR_NAME);
        let home = temp.path().join("custom").join(GATEWAY_HOME_DIR_NAME);
        let (pubkey, host_key) = write_enrolled_home(&predecessor);
        std::fs::rename(&predecessor, &home).unwrap();
        write_adoption_journal(
            &state,
            &AdoptionJournal {
                version: ADOPTION_JOURNAL_VERSION,
                status: AdoptionStatus::InProgress,
                predecessor_home: Some(predecessor.clone()),
                gateway_home: Some(home.clone()),
                completed: [
                    AdoptionStep::DatabaseFiles,
                    AdoptionStep::Ledger,
                    AdoptionStep::StewardSchedule,
                    AdoptionStep::GatewayHome,
                ]
                .into(),
                realm_id: None,
            },
        )
        .unwrap();
        let journal_before = std::fs::read(state.join(ADOPTION_JOURNAL_FILE_NAME)).unwrap();
        let other = temp.path().join("default").join(GATEWAY_HOME_DIR_NAME);

        // Different explicit homes: refused before any change.
        let conflicting = adopt_predecessor_state(&PredecessorAdoptRequest {
            state_dir: state.clone(),
            predecessor_home: None,
            gateway_home: Some(other.clone()),
            homes_explicit: true,
            mode: MigrateMode::Apply,
        })
        .await;
        assert!(conflicting.has_errors());
        assert!(!conflicting.changed_state);
        assert_eq!(
            std::fs::read(state.join(ADOPTION_JOURNAL_FILE_NAME)).unwrap(),
            journal_before
        );
        assert!(home.join(PREDECESSOR_DRAFT_STORE_FILE_NAME).is_file());

        // Omitted homes (derived defaults): the recorded homes are used and
        // the default home is never touched.
        let resumed = adopt_predecessor_state(&PredecessorAdoptRequest {
            state_dir: state.clone(),
            predecessor_home: None,
            gateway_home: Some(other.clone()),
            homes_explicit: false,
            mode: MigrateMode::Apply,
        })
        .await;
        assert!(!resumed.has_errors(), "{:?}", resumed.errors);
        assert!(resumed.complete);
        assert!(
            home.join(crate::mobpack::MOBPACK_DRAFT_STORE_FILENAME)
                .is_file()
        );
        assert!(!other.exists());
        assert_identity_preserved(&home, &pubkey, &host_key);
    }

    #[tokio::test]
    async fn malformed_or_live_registries_refuse_without_any_change() {
        let own_pid = std::process::id();
        for registry in [
            serde_json::json!({ "entries": {} }),
            serde_json::json!({}),
            serde_json::json!({ "entries": [{ "runtime_id": "r1" }] }),
            serde_json::json!({ "entries": [{ "pid": 4_000_000_000_u64, "runtime_id": 7 }] }),
            serde_json::json!({ "entries": [{ "pid": own_pid, "runtime_id": "live" }] }),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let state = temp.path();
            let predecessor = state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]);
            write_predecessor_metadata_db(&predecessor);
            std::fs::write(
                state.join(crate::storage_layout::RUNTIME_REGISTRY_FILE_NAME),
                serde_json::to_vec(&registry).unwrap(),
            )
            .unwrap();
            let before = snapshot(state);
            let report = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
            assert!(report.has_errors(), "{registry}: {report:?}");
            assert!(!report.changed_state, "{registry}");
            assert!(
                report
                    .actions
                    .iter()
                    .all(|action| action.item == PredecessorItem::Custody),
                "{registry}: {:?}",
                report.actions
            );
            assert_eq!(snapshot(state), before, "{registry}");
        }
    }

    /// A write that fails after its own stage exists (here the rename onto
    /// an occupied journal path) removes that stage, and only that stage.
    #[test]
    fn a_journal_write_failing_after_staging_removes_only_its_own_stage() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        // The journal path is an occupied directory: the rename must fail.
        let occupied = state.join(ADOPTION_JOURNAL_FILE_NAME);
        std::fs::create_dir_all(occupied.join("keep")).unwrap();
        let journal = AdoptionJournal {
            version: ADOPTION_JOURNAL_VERSION,
            status: AdoptionStatus::InProgress,
            predecessor_home: None,
            gateway_home: None,
            completed: BTreeSet::new(),
            realm_id: None,
        };
        let error = write_adoption_journal(state, &journal).expect_err("the rename fails");
        assert!(
            matches!(
                error,
                JournalWriteError::Staging {
                    left_behind: false,
                    ..
                }
            ),
            "{error}"
        );
        assert!(
            !staged_journal_path(state).exists(),
            "its own stage is removed"
        );
        assert!(occupied.join("keep").is_dir(), "nothing else is touched");
    }

    /// A stage that already exists is someone else's evidence: the helper
    /// refuses without touching it, and adoption refuses before any change.
    #[tokio::test]
    async fn a_preexisting_stage_is_preserved_and_refused() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        let predecessor = state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]);
        write_predecessor_metadata_db(&predecessor);
        let staged = staged_journal_path(state);
        std::fs::write(&staged, b"an earlier run's newest progress").unwrap();
        let before = snapshot(state);

        let journal = AdoptionJournal {
            version: ADOPTION_JOURNAL_VERSION,
            status: AdoptionStatus::InProgress,
            predecessor_home: None,
            gateway_home: None,
            completed: BTreeSet::new(),
            realm_id: None,
        };
        assert!(matches!(
            write_adoption_journal(state, &journal),
            Err(JournalWriteError::StagePresent { .. })
        ));
        assert_eq!(snapshot(state), before);

        let report = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(report.has_errors());
        assert!(!report.changed_state, "{report:?}");
        assert_eq!(
            snapshot(state),
            before,
            "the stage and every file keep their bytes"
        );
        assert!(!state.join(ADOPTION_JOURNAL_FILE_NAME).exists());
    }

    /// A composition manifest beside its event log, in a hand-written layout
    /// whose every byte the rename must keep. The recorded definition carries
    /// the creator field's name as nested data (a key, array elements and
    /// escaped text): none of it is the creator field.
    const PREDECESSOR_MANIFEST: &str = concat!(
        "{\n",
        "  \"manifest_version\": 1,\n",
        "  \"created_by_mobkit\":   \"0.8.26\",\n",
        "  \"definition\": {\"id\": \"m\", \"created_by_mobkit\": [\"created_by_mobkit\"], \"note\": \"a \\\"created_by_mobkit\\\", {not: a} key\"},\n",
        "  \"created_by_authority\": \"authoritative\"\n",
        "}\n",
    );

    fn adopted_manifest_text() -> String {
        let renamed = PREDECESSOR_MANIFEST.replacen(
            "\"created_by_mobkit\":   ",
            "\"created_by_meta\":   ",
            1,
        );
        assert_ne!(renamed, PREDECESSOR_MANIFEST);
        renamed
    }

    /// The stock manifest: beside the layout's event log.
    fn manifest_file(state: &Path) -> PathBuf {
        crate::mob_composition_manifest::manifest_path(
            &MetaStorageLayout::with_injected_roots(state.to_path_buf(), None).event_log_db(),
        )
    }

    fn manifest_actions(report: &PredecessorAdoptReport) -> Vec<&PredecessorAction> {
        report
            .actions
            .iter()
            .filter(|action| action.item == PredecessorItem::CompositionManifest)
            .collect()
    }

    /// The manifest step renames the top-level creator field and nothing
    /// else, is journaled, and a rerun finds nothing to do, its dry run
    /// included (the predecessor realm id, recorded by then, is not planned
    /// again).
    #[tokio::test]
    async fn a_predecessor_manifest_is_renamed_in_place_and_journaled() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        std::fs::create_dir(state.join(PREDECESSOR_REALM_ID)).unwrap();
        std::fs::write(manifest_file(state), PREDECESSOR_MANIFEST).unwrap();

        let dry = adopt_predecessor_state(&request(state, MigrateMode::DryRun)).await;
        assert!(!dry.has_errors(), "{:?}", dry.errors);
        let planned = manifest_actions(&dry);
        assert_eq!(planned.len(), 1, "{:?}", dry.actions);
        assert_eq!(planned[0].outcome, PredecessorOutcome::Planned);
        assert_eq!(
            planned[0].subject,
            manifest_file(state).display().to_string()
        );
        assert_eq!(planned[0].from, PREDECESSOR_MANIFEST_CREATOR_FIELD);
        assert_eq!(planned[0].to, MANIFEST_CREATOR_FIELD);
        assert_eq!(
            std::fs::read_to_string(manifest_file(state)).unwrap(),
            PREDECESSOR_MANIFEST,
            "the dry run is read-only"
        );

        let apply = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(!apply.has_errors(), "{:?}", apply.errors);
        assert!(apply.changed_state && apply.complete, "{apply:?}");
        let applied = manifest_actions(&apply);
        assert_eq!(applied.len(), 1, "{:?}", apply.actions);
        assert_eq!(applied[0].outcome, PredecessorOutcome::Applied);
        assert_eq!(
            std::fs::read_to_string(manifest_file(state)).unwrap(),
            adopted_manifest_text(),
            "only the top-level creator key changes"
        );
        assert!(!staged_manifest_path(&manifest_file(state)).exists());
        let journal = read_adoption_journal(state).unwrap().unwrap();
        assert_eq!(journal.status, AdoptionStatus::Complete);
        assert_eq!(journal.completed, BTreeSet::from(AdoptionStep::ALL));

        for (mode, label) in [
            (MigrateMode::DryRun, "dry run"),
            (MigrateMode::Apply, "apply"),
        ] {
            let again = adopt_predecessor_state(&request(state, mode)).await;
            assert!(!again.has_errors(), "{label}: {:?}", again.errors);
            assert!(again.actions.is_empty(), "{label}: {:?}", again.actions);
            assert!(again.complete && !again.changed_state, "{label}");
        }
        assert_eq!(
            std::fs::read_to_string(manifest_file(state)).unwrap(),
            adopted_manifest_text()
        );
    }

    /// An adoption that completed before the manifest step existed (0.9.0)
    /// leaves a predecessor manifest behind. Startup does not refuse it (the
    /// manifest reads its predecessor field), the dry run plans the rename
    /// and reports the adoption incomplete, and apply renames it instead of
    /// taking the already-complete shortcut.
    #[tokio::test]
    async fn a_manifest_found_after_a_completed_adoption_is_renamed_not_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_adoption_journal(
            state,
            &AdoptionJournal {
                version: ADOPTION_JOURNAL_VERSION,
                status: AdoptionStatus::Complete,
                predecessor_home: None,
                gateway_home: None,
                completed: AdoptionStep::ALL.into(),
                realm_id: None,
            },
        )
        .unwrap();
        std::fs::write(manifest_file(state), PREDECESSOR_MANIFEST).unwrap();
        MetaStorageLayout::with_injected_roots(state.to_path_buf(), None)
            .metadata_db()
            .expect("a predecessor manifest does not block startup");

        let dry = adopt_predecessor_state(&request(state, MigrateMode::DryRun)).await;
        assert!(!dry.has_errors(), "{:?}", dry.errors);
        assert!(!dry.complete, "a planned rename is not a complete adoption");
        assert_eq!(manifest_actions(&dry).len(), 1, "{:?}", dry.actions);

        let apply = adopt_predecessor_state(&request(state, MigrateMode::Apply)).await;
        assert!(!apply.has_errors(), "{:?}", apply.errors);
        assert!(apply.changed_state && apply.complete, "{apply:?}");
        assert_eq!(
            std::fs::read_to_string(manifest_file(state)).unwrap(),
            adopted_manifest_text()
        );
        let journal = read_adoption_journal(state).unwrap().unwrap();
        assert_eq!(journal.status, AdoptionStatus::Complete);
        assert_eq!(journal.completed, BTreeSet::from(AdoptionStep::ALL));
    }

    async fn assert_manifest_refused_unchanged(state: &Path, case: &str) {
        let before = snapshot(state);
        for mode in [MigrateMode::DryRun, MigrateMode::Apply] {
            let report = adopt_predecessor_state(&request(state, mode)).await;
            assert!(report.has_errors(), "{case}: {report:?}");
            assert!(!report.changed_state, "{case}: {report:?}");
            assert!(
                manifest_actions(&report)
                    .iter()
                    .any(|action| matches!(action.outcome, PredecessorOutcome::Refused(_))),
                "{case}: {:?}",
                report.actions
            );
        }
        assert_eq!(snapshot(state), before, "{case}: nothing changes");
        assert!(read_adoption_journal(state).unwrap().is_none(), "{case}");
    }

    /// A manifest the rename cannot judge refuses before any change: not a
    /// JSON object, no creator field, both creator fields, the predecessor
    /// field twice, an interrupted rename's stage beside it, and (on Unix) a
    /// link at the manifest's name.
    #[tokio::test]
    async fn a_manifest_that_cannot_be_judged_refuses_before_any_change() {
        let unjudgeable = [
            ("not a JSON object", "{\"created_by_mobkit\": \"0.8.26\""),
            (
                "no creator field",
                "{\"manifest_version\": 1, \"definition\": {}}",
            ),
            (
                "both creator fields",
                "{\"created_by_mobkit\": \"0.8.26\", \"created_by_meta\": \"0.9.1\"}",
            ),
            (
                "the predecessor field twice",
                "{\"created_by_mobkit\": \"0.8.26\", \"created_by_mobkit\": \"0.8.27\"}",
            ),
        ];
        for (case, bytes) in unjudgeable {
            let temp = tempfile::tempdir().unwrap();
            let state = temp.path();
            write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
            std::fs::write(manifest_file(state), bytes).unwrap();
            assert_manifest_refused_unchanged(state, case).await;
        }

        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        std::fs::write(manifest_file(state), PREDECESSOR_MANIFEST).unwrap();
        std::fs::write(staged_manifest_path(&manifest_file(state)), b"{\"partial").unwrap();
        assert_manifest_refused_unchanged(state, "an interrupted rename's stage").await;

        #[cfg(unix)]
        {
            let temp = tempfile::tempdir().unwrap();
            let state = temp.path();
            write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
            let target = state.join("linked-manifest.json");
            std::fs::write(&target, PREDECESSOR_MANIFEST).unwrap();
            std::os::unix::fs::symlink(&target, manifest_file(state)).unwrap();
            assert_manifest_refused_unchanged(state, "a link").await;
        }
    }

    /// The rename rechecks the bytes it planned: a manifest changed since
    /// planning fails the step, is never overwritten, and no stage is left.
    #[test]
    fn the_manifest_rename_rechecks_its_bytes_and_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        std::fs::write(manifest_file(state), PREDECESSOR_MANIFEST).unwrap();
        let mut report = PredecessorAdoptReport::default();
        let plan = plan_state(state, &mut report).expect("the manifest plans");
        assert_eq!(plan.composition_manifests.len(), 1);
        let changed = PREDECESSOR_MANIFEST.replace("0.8.26", "0.8.27");
        std::fs::write(manifest_file(state), &changed).unwrap();
        assert!(!apply_composition_manifests(&plan, &mut report));
        assert!(matches!(
            report.actions.last().map(|action| &action.outcome),
            Some(PredecessorOutcome::Failed(_))
        ));
        assert_eq!(
            std::fs::read_to_string(manifest_file(state)).unwrap(),
            changed
        );
        assert!(!staged_manifest_path(&manifest_file(state)).exists());
    }

    /// The repair entry point detects, renames, then finds the manifest
    /// current, also for a predecessor key written with a JSON escape; the
    /// manifest reads the same creator version before and after.
    #[test]
    fn repair_detects_renames_and_then_finds_the_manifest_current() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = temp.path().join("mob.sqlite3.composition.json");
        let definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"repair\"\n\n[profiles.lead]\nmodel = \"gpt-5.5\"\n",
        )
        .unwrap();
        let escaped_key = "created_by_mobki\\u0074";
        let bytes = format!(
            "{{\"manifest_version\":1,\"{escaped_key}\":\"0.8.26\",\"definition\":{},\
             \"created_by_authority\":\"authoritative\"}}",
            serde_json::to_string(&definition).unwrap()
        );
        std::fs::write(&manifest, &bytes).unwrap();
        let creator = |path: &Path| {
            serde_json::from_slice::<crate::mob_composition_manifest::MobCompositionManifest>(
                &std::fs::read(path).unwrap(),
            )
            .unwrap()
            .created_by_meta
        };
        assert_eq!(creator(&manifest), "0.8.26");

        assert_eq!(
            repair_composition_manifest(&manifest, MigrateMode::DryRun),
            Ok(ManifestRepair::Predecessor)
        );
        assert_eq!(
            std::fs::read_to_string(&manifest).unwrap(),
            bytes,
            "detection is read-only"
        );
        assert_eq!(
            repair_composition_manifest(&manifest, MigrateMode::Apply),
            Ok(ManifestRepair::Rewritten)
        );
        assert_eq!(
            std::fs::read_to_string(&manifest).unwrap(),
            bytes.replace(escaped_key, MANIFEST_CREATOR_FIELD)
        );
        assert_eq!(creator(&manifest), "0.8.26");
        assert_eq!(
            repair_composition_manifest(&manifest, MigrateMode::Apply),
            Ok(ManifestRepair::Current)
        );
    }

    /// The stage that receives the renamed manifest is owner-only from its
    /// creation, before any byte is written, so the manifest's contents are
    /// never readable to others before its own permissions apply; the
    /// replaced manifest keeps its original mode.
    #[cfg(unix)]
    #[test]
    fn the_rename_stage_is_owner_only_and_the_manifest_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        let probe = temp.path().join("probe.composition.json.next");
        let stage = create_manifest_stage(&probe).unwrap();
        assert_eq!(
            stage.metadata().unwrap().permissions().mode() & 0o777,
            0o600,
            "the stage is created owner-only"
        );
        drop(stage);
        for mode in [0o600_u32, 0o640, 0o644] {
            let manifest = temp
                .path()
                .join(format!("mob-{mode:o}.sqlite3.composition.json"));
            std::fs::write(&manifest, PREDECESSOR_MANIFEST).unwrap();
            std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(
                repair_composition_manifest(&manifest, MigrateMode::Apply),
                Ok(ManifestRepair::Rewritten)
            );
            assert_eq!(
                std::fs::metadata(&manifest).unwrap().permissions().mode() & 0o777,
                mode,
                "the manifest keeps mode {mode:o}"
            );
            assert_eq!(
                std::fs::read_to_string(&manifest).unwrap(),
                adopted_manifest_text()
            );
        }
    }

    /// A failure after the renamed manifest replaced the original (here the
    /// directory sync) is reported failed, never refused: the replacement
    /// happened and its durability is uncertain. A refusal still changes
    /// nothing.
    #[test]
    fn a_failure_after_the_replacement_is_failed_not_refused() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = temp.path().join("mob.sqlite3.composition.json");
        std::fs::write(&manifest, PREDECESSOR_MANIFEST).unwrap();
        let error = repair_composition_manifest_with(&manifest, MigrateMode::Apply, |_: &Path| {
            Err("injected directory sync failure".to_string())
        })
        .expect_err("the directory sync failure surfaces");
        assert_eq!(error.outcome(), "failed", "{error:?}");
        assert!(
            matches!(
                &error,
                ManifestRepairError::Failed(detail)
                    if detail.contains("injected directory sync failure")
                        && detail.contains("not yet durable")
            ),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&manifest).unwrap(),
            adopted_manifest_text(),
            "the replacement happened"
        );
        assert!(!staged_manifest_path(&manifest).exists());

        let ambiguous = temp.path().join("other.sqlite3.composition.json");
        let both = "{\"created_by_mobkit\": \"0.8.26\", \"created_by_meta\": \"0.9.1\"}";
        std::fs::write(&ambiguous, both).unwrap();
        let refusal =
            repair_composition_manifest_with(&ambiguous, MigrateMode::Apply, |_: &Path| Ok(()))
                .expect_err("an ambiguous manifest refuses");
        assert_eq!(refusal.outcome(), "refused", "{refusal:?}");
        assert_eq!(std::fs::read_to_string(&ambiguous).unwrap(), both);
    }

    /// A mob definition as a predecessor release recorded it: one profile,
    /// with nothing beyond what every release carried.
    fn recorded_definition() -> MobDefinition {
        MobDefinition::from_toml("[mob]\nid = \"m\"\n\n[profiles.lead]\nmodel = \"gpt-5.5\"\n")
            .unwrap()
    }

    /// The same definition with the three keys predecessor releases before
    /// 0.8.34 ignored: a mob-wide and a profile model fallback, and a tool
    /// deny list.
    fn definition_with_ignored_keys() -> MobDefinition {
        MobDefinition::from_toml(
            "[mob]\nid = \"m\"\n\n[runtime.model_fallback]\nenabled = false\n\n\
             [profiles.lead]\nmodel = \"gpt-5.5\"\n\n[profiles.lead.model_fallback]\n\
             enabled = false\n\n[profiles.lead.tools]\ndeny = [\"shell\"]\n",
        )
        .unwrap()
    }

    const IGNORED_KEYS: [&str; 3] = [
        "profiles.lead.model_fallback",
        "profiles.lead.tools.deny",
        "runtime.model_fallback",
    ];

    fn key_paths(paths: &[&str]) -> Option<Vec<String>> {
        Some(paths.iter().map(|path| (*path).to_string()).collect())
    }

    /// The stock composition manifest as predecessor release `release` wrote
    /// it, recording `definition`, with no mob storage beside it.
    fn write_recorded_manifest(state: &Path, release: &str, definition: &MobDefinition) {
        let manifest = serde_json::json!({
            "manifest_version": 1,
            "created_by_mobkit": release,
            "definition": definition,
            "created_by_authority": "authoritative",
        });
        std::fs::write(
            manifest_file(state),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn definition_check(
        definition: MobDefinition,
        acknowledge_ignored_keys: bool,
    ) -> PredecessorDefinitionCheck {
        PredecessorDefinitionCheck::new(
            vec![("mob.toml".to_string(), definition)],
            acknowledge_ignored_keys,
        )
    }

    /// The definition the released MobKit 0.8.26 store was created with (see
    /// tests/fixtures/README.md): mob `adoption-fixture-planners`, profile
    /// `planner`.
    const RELEASED_MOB_CONFIG: &str = "[mob]\nid = \"adoption-fixture-planners\"\n\n\
        [profiles.planner]\nmodel = \"gpt-5.5\"\nexternal_addressable = true\n\
        runtime_mode = \"turn_driven\"\n\n[profiles.planner.tools]\ncomms = true\n";

    /// The released definition with a tool deny list, and optionally with
    /// both model fallbacks: keys 0.8.26 ignored.
    fn released_definition_with(deny: bool, model_fallbacks: bool) -> MobDefinition {
        let mut config = RELEASED_MOB_CONFIG.to_string();
        if deny {
            config.push_str("deny = [\"shell\"]\n");
        }
        if model_fallbacks {
            config.push_str(
                "\n[profiles.planner.model_fallback]\nenabled = false\n\n\
                 [runtime.model_fallback]\nenabled = false\n",
            );
        }
        MobDefinition::from_toml(&config).unwrap()
    }

    /// The released 0.8.26 store's mob storage and its composition manifest,
    /// copied byte for byte into `state`: a real predecessor event log that
    /// holds the released definition. The fixture itself is never opened.
    fn stage_released_mob_storage(state: &Path) {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/released_0_8_26_store/state");
        // The released WAL is empty: the database file holds every event.
        assert_eq!(
            std::fs::metadata(fixture.join("event_log.sqlite3-wal"))
                .unwrap()
                .len(),
            0
        );
        for name in ["event_log.sqlite3", "event_log.sqlite3.composition.json"] {
            std::fs::copy(fixture.join(name), state.join(name)).unwrap();
        }
        assert_eq!(
            manifest_file(state),
            state.join("event_log.sqlite3.composition.json")
        );
    }

    /// Advance the stored definition of the mob storage in `state` once, as
    /// a later release's definition update does, and wait for its store to
    /// close.
    async fn advance_stored_definition(state: &Path, definition: MobDefinition) {
        let path = state.join("event_log.sqlite3");
        let storage = meerkat_mob::MobStorage::persistent(&path).unwrap();
        let revision = storage.update_definition(1, definition).await.unwrap();
        assert_eq!(revision.epoch, 2, "{revision:?}");
        drop(storage);
        // The store's connection worker closes its connection on its own
        // thread after the store is dropped; the exclusive fence waits for
        // it, so planning reads a quiescent file.
        drop(
            meerkat_sqlite::ExclusiveFence::acquire(&path, std::time::Duration::from_secs(30))
                .expect("mob store worker closed"),
        );
    }

    /// The keys reported are those the creating release ignored, by the
    /// first release whose definition carried each (`model_fallback` from
    /// 0.8.34, `tools.deny` from 0.8.46), that the held definition does not
    /// record. A key it records, an unchanged definition and a profile it
    /// lacks are not reported (the profile is noted instead), and a creator
    /// that is not a release version is not judged.
    #[test]
    fn the_reported_keys_are_those_the_creating_release_ignored_and_the_store_lacks() {
        let held = recorded_definition();
        let supplied = definition_with_ignored_keys();
        for (release, expected) in [
            ("0.8.9", key_paths(&IGNORED_KEYS)),
            ("0.8.33", key_paths(&IGNORED_KEYS)),
            ("0.8.34", key_paths(&["profiles.lead.tools.deny"])),
            ("0.8.45", key_paths(&["profiles.lead.tools.deny"])),
            ("0.8.46", key_paths(&[])),
            ("0.8.49", key_paths(&[])),
            ("0.9.1", key_paths(&[])),
        ] {
            assert_eq!(
                predecessor_ignored_keys(release, &held, &supplied),
                expected,
                "{release}"
            );
        }
        assert_eq!(
            predecessor_ignored_keys("0.8.26", &held, &held),
            key_paths(&[]),
            "an unchanged definition"
        );
        assert_eq!(
            predecessor_ignored_keys("0.8.26", &supplied, &supplied),
            key_paths(&[]),
            "keys the held definition records, whichever release advanced it"
        );
        let new_profile = MobDefinition::from_toml(
            "[mob]\nid = \"m\"\n\n[profiles.lead]\nmodel = \"gpt-5.5\"\n\n\
             [profiles.other]\nmodel = \"gpt-5.5\"\n\n[profiles.other.tools]\ndeny = [\"shell\"]\n",
        )
        .unwrap();
        assert_eq!(
            predecessor_ignored_keys("0.8.26", &held, &new_profile),
            key_paths(&[]),
            "a profile the held definition lacks is not compared"
        );
        assert_eq!(
            profiles_not_held("0.8.26", &held, &new_profile),
            vec![(
                "other".to_string(),
                vec!["profiles.other.tools.deny".to_string()]
            )]
        );
        assert_eq!(profiles_not_held("0.8.46", &held, &new_profile), Vec::new());
        assert_eq!(
            profiles_not_held("0.8.26", &new_profile, &new_profile),
            Vec::new()
        );
        for creator in ["0.8", "0.8.26.1", "0.8.26-dev", "mobkit"] {
            assert_eq!(
                predecessor_ignored_keys(creator, &held, &supplied),
                None,
                "{creator}"
            );
        }
    }

    /// Over the released 0.8.26 store, keys 0.8.26 ignored, that Meta 0.9
    /// applies and that the store's definition does not record are named and
    /// refused before any change, by the dry run and by apply, until they
    /// are acknowledged; adoption then completes and still reports them. The
    /// released definition reports nothing.
    #[tokio::test]
    async fn keys_the_store_does_not_hold_refuse_until_acknowledged() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        stage_released_mob_storage(state);
        let before = snapshot(state);
        let manifest = manifest_file(state).display().to_string();
        let keys = [
            "profiles.planner.model_fallback",
            "profiles.planner.tools.deny",
            "runtime.model_fallback",
        ];
        let reported = |acknowledged| {
            vec![PredecessorIgnoredKeys {
                manifest: manifest.clone(),
                mob_id: "adoption-fixture-planners".to_string(),
                source: "mob.toml".to_string(),
                created_by: "0.8.26".to_string(),
                keys: keys.map(String::from).to_vec(),
                acknowledged,
            }]
        };

        let unchanged = adopt_predecessor_state_with_definitions(
            &request(state, MigrateMode::DryRun),
            &definition_check(released_definition_with(false, false), false),
        )
        .await;
        assert!(!unchanged.has_errors(), "{:?}", unchanged.errors);
        assert_eq!(unchanged.ignored_keys, Vec::new());

        for mode in [MigrateMode::DryRun, MigrateMode::Apply] {
            let report = adopt_predecessor_state_with_definitions(
                &request(state, mode),
                &definition_check(released_definition_with(true, true), false),
            )
            .await;
            assert_eq!(report.ignored_keys, reported(false), "{mode:?}");
            assert!(!report.changed_state, "{mode:?}");
            assert_eq!(report.errors.len(), 1, "{mode:?}: {:?}", report.errors);
            let error = &report.errors[0];
            assert!(
                error.starts_with(&format!("refused, nothing changed: {manifest}: ")),
                "{error}"
            );
            for needle in keys.into_iter().chain([
                "predecessor release 0.8.26",
                "mob.toml",
                "an authoritative resume",
                "--acknowledge-ignored-keys",
            ]) {
                assert!(error.contains(needle), "{needle:?} missing from {error}");
            }
        }
        assert_eq!(snapshot(state), before, "a refusal changes nothing");
        assert!(read_adoption_journal(state).unwrap().is_none());

        let applied = adopt_predecessor_state_with_definitions(
            &request(state, MigrateMode::Apply),
            &definition_check(released_definition_with(true, true), true),
        )
        .await;
        assert!(!applied.has_errors(), "{:?}", applied.errors);
        assert!(applied.changed_state && applied.complete, "{applied:?}");
        assert_eq!(applied.ignored_keys, reported(true));
        assert!(
            applied
                .notes
                .iter()
                .any(|note| note.contains("acknowledged")
                    && note.contains(keys[1])
                    && note.contains("an authoritative resume")),
            "{:?}",
            applied.notes
        );
    }

    /// A store 0.8.26 created whose definition a later release advanced with
    /// a tool deny list runs with it: adoption does not name the deny list
    /// (removing it would drop a list in force, and make the store's resume
    /// refuse), and still names the model fallbacks the advanced definition
    /// does not record.
    #[tokio::test]
    async fn a_key_the_advanced_definition_records_is_in_force_and_not_named() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        stage_released_mob_storage(state);
        advance_stored_definition(state, released_definition_with(true, false)).await;

        let with_fallbacks = adopt_predecessor_state_with_definitions(
            &request(state, MigrateMode::DryRun),
            &definition_check(released_definition_with(true, true), false),
        )
        .await;
        assert_eq!(
            with_fallbacks
                .ignored_keys
                .iter()
                .map(|ignored| ignored.keys.clone())
                .collect::<Vec<_>>(),
            vec![vec![
                "profiles.planner.model_fallback".to_string(),
                "runtime.model_fallback".to_string()
            ]]
        );
        assert_eq!(
            with_fallbacks.errors.len(),
            1,
            "{:?}",
            with_fallbacks.errors
        );

        for mode in [MigrateMode::DryRun, MigrateMode::Apply] {
            let deny_in_force = adopt_predecessor_state_with_definitions(
                &request(state, mode),
                &definition_check(released_definition_with(true, false), false),
            )
            .await;
            assert!(
                !deny_in_force.has_errors(),
                "{mode:?}: {:?}",
                deny_in_force.errors
            );
            assert_eq!(deny_in_force.ignored_keys, Vec::new(), "{mode:?}");
        }
    }

    /// Without definitions, a store whose creating release ignored keys is
    /// noted with the flag that checks it. A definition that names no store,
    /// a store no definition names, a manifest with no mob storage beside it
    /// and a profile the store's definition lacks are noted, never refused.
    /// A manifest that does not decode cannot be checked: refused when
    /// definitions are given, and adopted as before when none are.
    #[tokio::test]
    async fn unchecked_and_unmatched_definitions_are_noted_not_refused() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        write_predecessor_metadata_db(&state.join(PREDECESSOR_METADATA_DB_FILE_NAMES[0]));
        write_recorded_manifest(state, "0.8.40", &recorded_definition());

        let unchecked = adopt_predecessor_state(&request(state, MigrateMode::DryRun)).await;
        assert!(!unchecked.has_errors(), "{:?}", unchecked.errors);
        assert_eq!(unchecked.ignored_keys, Vec::new());
        assert!(
            unchecked
                .notes
                .iter()
                .any(|note| note.contains("--mob-config")
                    && note.contains("predecessor release 0.8.40")
                    && note.contains("tools.deny")
                    && !note.contains("model_fallback")),
            "{:?}",
            unchecked.notes
        );

        let no_storage = adopt_predecessor_state_with_definitions(
            &request(state, MigrateMode::DryRun),
            &definition_check(definition_with_ignored_keys(), false),
        )
        .await;
        assert!(!no_storage.has_errors(), "{:?}", no_storage.errors);
        assert_eq!(no_storage.ignored_keys, Vec::new());
        assert!(
            no_storage
                .notes
                .iter()
                .any(|note| note.contains("no mob storage beside it holds a mob")),
            "{:?}",
            no_storage.notes
        );

        write_recorded_manifest(state, "0.8.46", &recorded_definition());
        let current = adopt_predecessor_state(&request(state, MigrateMode::DryRun)).await;
        assert!(
            !current
                .notes
                .iter()
                .any(|note| note.contains("--mob-config")),
            "a release that ignored nothing is not noted: {:?}",
            current.notes
        );

        stage_released_mob_storage(state);
        let other = MobDefinition::from_toml(
            "[mob]\nid = \"other\"\n\n[profiles.lead]\nmodel = \"gpt-5.5\"\n\n\
             [profiles.lead.tools]\ndeny = [\"shell\"]\n",
        )
        .unwrap();
        let unmatched = adopt_predecessor_state_with_definitions(
            &request(state, MigrateMode::DryRun),
            &definition_check(other, false),
        )
        .await;
        assert!(!unmatched.has_errors(), "{:?}", unmatched.errors);
        assert_eq!(unmatched.ignored_keys, Vec::new());
        for needle in ["holds mob other", "names mob adoption-fixture-planners"] {
            assert!(
                unmatched.notes.iter().any(|note| note.contains(needle)),
                "{needle:?} missing from {:?}",
                unmatched.notes
            );
        }

        let mut added = released_definition_with(false, false);
        added.profiles.insert(
            "reviewer".into(),
            MobDefinition::from_toml(
                "[mob]\nid = \"x\"\n\n[profiles.reviewer]\nmodel = \"gpt-5.5\"\n\n\
                 [profiles.reviewer.tools]\ndeny = [\"shell\"]\n",
            )
            .unwrap()
            .profiles
            .remove("reviewer")
            .unwrap(),
        );
        let new_profile = adopt_predecessor_state_with_definitions(
            &request(state, MigrateMode::DryRun),
            &definition_check(added, false),
        )
        .await;
        assert!(!new_profile.has_errors(), "{:?}", new_profile.errors);
        assert_eq!(new_profile.ignored_keys, Vec::new());
        assert!(
            new_profile.notes.iter().any(|note| note
                .contains("profile reviewer in mob.toml sets profiles.reviewer.tools.deny")),
            "{:?}",
            new_profile.notes
        );

        std::fs::write(
            manifest_file(state),
            "{\"manifest_version\": 1, \"created_by_mobkit\": \"0.8.26\", \"definition\": {\"profiles\": 1}}",
        )
        .unwrap();
        let undecodable = adopt_predecessor_state_with_definitions(
            &request(state, MigrateMode::DryRun),
            &definition_check(recorded_definition(), false),
        )
        .await;
        assert!(
            undecodable
                .errors
                .iter()
                .any(|error| error.contains("cannot check the supplied mob definitions")),
            "{:?}",
            undecodable.errors
        );
        let adopted_as_before = adopt_predecessor_state(&request(state, MigrateMode::DryRun)).await;
        assert!(
            !adopted_as_before.has_errors(),
            "{:?}",
            adopted_as_before.errors
        );
    }
}

//! The single Meta path authority for storage roots and canonical
//! top-level database locators.
//!
//! [`MetaStorageLayout`] is constructed once at bootstrap and carried
//! through composition; no surface derives state-dir file names or resolves
//! ambient roots on its own. The layout owns **roots and canonical
//! top-level locators**; feature crates own *relative* names beneath them —
//! the workgraph admission sidecar's lock name, the blob directory's
//! internal sharding, and per-realm agent-memory files stay feature-owned.
//!
//! The two sanctioned ambient derivations live in this module and nowhere
//! else (the M5 anti-ambient-resolution gate allowlists exactly this file):
//! [`default_gateway_home`] (`$XDG_STATE_HOME`/`$HOME` rules) and
//! [`default_ephemeral_scratch_root`] (per-process root under the OS temp
//! directory for explicitly declared-ephemeral layouts).
//!
//! # Canonical spellings
//!
//! Decided here, once (storage-unification plan, Phase M2). Stores shared
//! with Meerkat keep Meerkat's names; Meta-owned files converge on the
//! `*.sqlite3` convention. Legacy spellings remain readable through probing.
//!
//! | Slot | Canonical | Legacy spellings |
//! |---|---|---|
//! | sessions | `sessions.sqlite3` (Meerkat's realm spelling) | `sessions.db`, `sessions.sqlite` |
//! | runtime | `runtime.sqlite` | — |
//! | schedule | [`SCHEDULE_STORE_FILE`] (`schedule.sqlite`) | — |
//! | workgraph | [`WORKGRAPH_STORE_FILE`] (`workgraph.sqlite3`) | — |
//! | continuity | `continuity.sqlite3` | `continuity.db`, `identity_continuity.sqlite` |
//! | metadata | `meta_metadata.sqlite3` | none |
//! | console | `meta_console.sqlite3` | none |
//! | agent-memory root | `agent-memory/` | `agent-memory-sqlite/` |
//! | event log | `event_log.sqlite3` (reserved; nothing opens it pre-M4) | — |
//! | blob root | `blobs/` | — |
//! | peer key | `peer_key.ed25519` (gateway home) | — |
//! | registry | `tux-runtimes.json` (gateway home) | — |
//!
//! # Canonical-name-first probing
//!
//! [`MetaStorageLayout::resolve_database`] resolves the canonical name,
//! then probes the known legacy spellings in the same directory:
//!
//! - exactly one spelling exists → use it **where it lies** (no rename at
//!   open; physical renames arrive only with the M6 migration verb, under
//!   the maintenance fence, with changelog entries);
//! - canonical AND a legacy spelling exist, or two legacy spellings exist →
//!   [`StorageLayoutError::FileNameTwins`], pointing at the storage doctor;
//! - none exists → the canonical name (a fresh deployment converges).
//!
//! The invariant: the resolver never *creates* a twin.
//!
//! # Predecessor names
//!
//! Before 0.9 the metadata and console files, the gateway's memory ledger
//! file, the ledger domains and the gateway home carried the predecessor
//! product name ([`crate::storage_predecessor`]). They are not legacy
//! spellings: no resolver probes them and no store opens them. A state
//! directory (or gateway home) that still holds one refuses typed
//! ([`StorageLayoutError::PredecessorState`],
//! [`StorageLayoutError::PredecessorGatewayHome`]) until the explicit
//! one-shot `meta_gateway storage-adopt-predecessor` verb has carried it
//! over, so a renamed default can never silently start empty beside
//! existing state. An adoption that changed state but has not finished
//! refuses the same way ([`StorageLayoutError::AdoptionIncomplete`]): its
//! journal, not the absence of predecessor names, is the completion
//! evidence.

use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::auth::peer_keys::KEY_FILE_NAME;
use crate::council_wiring::COUNCIL_STORE_FILE;
use crate::schedule_wiring::SCHEDULE_STORE_FILE;
use crate::workgraph_wiring::WORKGRAPH_STORE_FILE;

/// Canonical sessions database file name (Meerkat's realm spelling).
pub const CANONICAL_SESSIONS_DB_FILE_NAME: &str = "sessions.sqlite3";
/// Runtime store file name (all three surfaces already agree; kept).
pub const RUNTIME_DB_FILE_NAME: &str = "runtime.sqlite";
/// Canonical identity-continuity database file name.
pub const CANONICAL_CONTINUITY_DB_FILE_NAME: &str = "continuity.sqlite3";
/// Canonical runtime-metadata database file name.
pub const CANONICAL_METADATA_DB_FILE_NAME: &str = "meta_metadata.sqlite3";
/// Canonical console-timeline database file name.
pub const CANONICAL_CONSOLE_DB_FILE_NAME: &str = "meta_console.sqlite3";
/// Canonical agent-memory root directory name (per-realm files beneath it
/// stay feature-owned).
pub const CANONICAL_AGENT_MEMORY_DIR_NAME: &str = "agent-memory";
/// The extension document store's database file (extension-state feature).
/// Named in every build: a state directory an extension-state embedder wrote
/// is fenced and adopted by the stock gateway too.
pub const EXTENSION_STATE_DB_FILE_NAME: &str = "extension_state.sqlite3";
/// The mob announcement database file. Not a [`DatabaseSlot`]: it is new in
/// 0.9.4 and has no legacy spelling to reconcile. Opened only when a gateway
/// enables mob announcements; storage maintenance fences it in every build.
pub const MOB_ANNOUNCEMENTS_DB_FILE_NAME: &str = "mob_announcements.sqlite3";
/// Reserved event-log database file name. Nothing opens it before the M4
/// disk factory; the locator exists so the name is decided exactly once.
pub const EVENT_LOG_DB_FILE_NAME: &str = "event_log.sqlite3";
/// Blob root directory name (internal sharding stays feature-owned).
pub const BLOB_ROOT_DIR_NAME: &str = "blobs";
/// Runtime registry file name under the gateway home.
pub const RUNTIME_REGISTRY_FILE_NAME: &str = "tux-runtimes.json";
/// Directory name appended to the XDG state root for the gateway home.
pub const GATEWAY_HOME_DIR_NAME: &str = "meerkat-meta";
/// The stock gateway's operational memory ledger file in its state
/// directory: `rpc_gateway` persists the `local_json` ledger at
/// `<persistent_state>/<this name>`.
pub const MEMORY_LEDGER_FILE_NAME: &str = "memory-ledger-state.json";
/// The lock file in a state directory through which one Meta runtime holds
/// the directory (see [`crate::state_dir_claim`]).
pub const STATE_DIR_LOCK_FILE_NAME: &str = "meta-runtime.lock";

/// The one-shot offline verb that moves a pre-0.9.4 state directory onto
/// Meerkat's native session storage.
pub const NATIVE_STORAGE_MIGRATION_VERB: &str = "meta_gateway storage-migrate-native";

/// The verb that moves a separate session database into a state directory's
/// layout (named by [`StorageLayoutError::SessionDatabaseOverrideRetired`]).
pub const SESSION_DATABASE_IMPORT_VERB: &str = "meta_gateway storage-migrate";

/// Where an operator reads how to run [`NATIVE_STORAGE_MIGRATION_VERB`]
/// (and, for an SDK-hosted 0.9.3 installation, the export that precedes it).
pub const NATIVE_STORAGE_MIGRATION_GUIDE: &str =
    "docs/guides/upgrading-to-0-9-4.mdx#continuity-stores-and-lease-providers-are-removed";

/// The nine top-level database locators the layout owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseSlot {
    Sessions,
    Runtime,
    Schedule,
    Workgraph,
    Continuity,
    Metadata,
    Console,
    AgentMemory,
    EventLog,
    Council,
    #[cfg(feature = "extension-state")]
    ExtensionState,
}

impl DatabaseSlot {
    /// Every slot, in the order the layout summary reports them.
    pub const ALL: [Self; 10 + cfg!(feature = "extension-state") as usize] = [
        Self::Sessions,
        Self::Runtime,
        Self::Schedule,
        Self::Workgraph,
        Self::Continuity,
        Self::Metadata,
        Self::Console,
        Self::AgentMemory,
        Self::EventLog,
        Self::Council,
        #[cfg(feature = "extension-state")]
        Self::ExtensionState,
    ];

    /// The canonical file (or directory) name for this slot.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Sessions => CANONICAL_SESSIONS_DB_FILE_NAME,
            Self::Runtime => RUNTIME_DB_FILE_NAME,
            Self::Schedule => SCHEDULE_STORE_FILE,
            Self::Workgraph => WORKGRAPH_STORE_FILE,
            Self::Continuity => CANONICAL_CONTINUITY_DB_FILE_NAME,
            Self::Metadata => CANONICAL_METADATA_DB_FILE_NAME,
            Self::Console => CANONICAL_CONSOLE_DB_FILE_NAME,
            Self::AgentMemory => CANONICAL_AGENT_MEMORY_DIR_NAME,
            Self::EventLog => EVENT_LOG_DB_FILE_NAME,
            Self::Council => COUNCIL_STORE_FILE,
            #[cfg(feature = "extension-state")]
            Self::ExtensionState => EXTENSION_STATE_DB_FILE_NAME,
        }
    }

    /// The known legacy spellings probed beside the canonical name.
    pub fn legacy_names(self) -> &'static [&'static str] {
        match self {
            Self::Sessions => &["sessions.db", "sessions.sqlite"],
            Self::Continuity => &["continuity.db", "identity_continuity.sqlite"],
            Self::AgentMemory => &["agent-memory-sqlite"],
            #[cfg(feature = "extension-state")]
            Self::ExtensionState => &[],
            Self::Metadata
            | Self::Console
            | Self::Runtime
            | Self::Schedule
            | Self::Workgraph
            | Self::EventLog
            | Self::Council => &[],
        }
    }
}

impl Display for DatabaseSlot {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Sessions => "sessions",
            Self::Runtime => "runtime",
            Self::Schedule => "schedule",
            Self::Workgraph => "workgraph",
            Self::Continuity => "continuity",
            Self::Metadata => "metadata",
            Self::Console => "console",
            Self::AgentMemory => "agent-memory",
            Self::EventLog => "event-log",
            Self::Council => "council",
            #[cfg(feature = "extension-state")]
            Self::ExtensionState => "extension-state",
        };
        f.write_str(name)
    }
}

/// How a database locator resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "name")]
pub enum DatabaseProvenance {
    /// The canonical spelling (existing, or fresh — nothing else existed).
    Canonical,
    /// A known legacy spelling, used where it lies (never renamed at open).
    LegacySpelling(String),
    /// The caller supplied an explicit database file (the gateway
    /// `store_path`-with-extension escape hatch); probing is bypassed.
    ExplicitOverride,
}

/// A resolved database locator: the path to open plus how it was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDatabase {
    pub path: PathBuf,
    pub provenance: DatabaseProvenance,
}

/// Whether the layout's state root is durable or an explicitly declared
/// scratch root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateDirDurability {
    Durable,
    DeclaredEphemeral,
}

/// Typed layout refusals.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StorageLayoutError {
    /// Two spellings of the same store exist in the state directory. The
    /// resolver refuses to choose (opening one would fork history; renaming
    /// at open is the M6 migration verb's job, under the fence).
    FileNameTwins {
        slot: DatabaseSlot,
        paths: Vec<PathBuf>,
    },
    /// Neither `$XDG_STATE_HOME` nor `$HOME` is available to derive the
    /// gateway home.
    GatewayHomeUnavailable,
    /// The state directory still holds predecessor (pre-0.9) state. Opening
    /// anything would start the renamed stores empty beside it; the explicit
    /// adoption verb carries it over first. `state_dir` is the directory the
    /// refusing layout resolved: the exact `--state-dir` that adoption
    /// needs, never inferred by the adoption verb itself.
    PredecessorState {
        state_dir: PathBuf,
        paths: Vec<PathBuf>,
    },
    /// The derived gateway home does not exist but its predecessor
    /// (pre-0.9) sibling does. Minting a fresh home would mint a new peer
    /// key that every pinned peer rejects, and drop the host pairings and
    /// flow-editor drafts.
    PredecessorGatewayHome { home: PathBuf, predecessor: PathBuf },
    /// A predecessor adoption changed this installation and has not
    /// finished: its journal records unfinished steps, or the gateway home
    /// still carries a predecessor name. A rerun of the adoption resumes it.
    AdoptionIncomplete { path: PathBuf, detail: String },
    /// The adoption journal exists but cannot be read or decoded (for
    /// example torn by a crash on a filesystem that reordered the write).
    /// Startup and the adoption verb refuse until it is repaired by hand.
    AdoptionJournalUnreadable { path: PathBuf, detail: String },
    /// Both the predecessor and the Meta meerkat-level realm directories
    /// exist. Picking either would silently orphan the other's jobs.
    RealmTwin { paths: Vec<PathBuf> },
    /// An adopted installation's recorded realm id is missing or invalid.
    /// Never replaced by a default.
    AdoptedRealmInvalid {
        journal: PathBuf,
        value: Option<String>,
        reason: String,
    },
    /// Another Meta runtime, in this process or another, holds the state
    /// directory (see [`crate::state_dir_claim`]).
    StateDirInUse { state_dir: PathBuf },
    /// The state directory or its lock file could not be created or locked.
    StateDirUnavailable { state_dir: PathBuf, detail: String },
    /// Another process still hosts runtimes on the state directory: Core's
    /// execution-custody lock on `runtime.sqlite` is held (see
    /// [`crate::native_startup`]).
    RuntimeInUse { state_dir: PathBuf, file: PathBuf },
    /// Whether another process hosts runtimes on the state directory could
    /// not be checked.
    RuntimeLockUnreadable {
        state_dir: PathBuf,
        file: PathBuf,
        detail: String,
    },
    /// The state directory's stores still have the 0.9.3 layout:
    /// [`NATIVE_STORAGE_MIGRATION_VERB`] converts them first.
    NativeStorageMigrationRequired {
        state_dir: PathBuf,
        status: crate::storage_migrate_native::NativeLayoutStatus,
    },
    /// Identity bindings name sessions, but `runtime.sqlite` is missing or
    /// holds no session: the runtime store was lost or replaced.
    RuntimeStoreMissing {
        state_dir: PathBuf,
        bound_identities: usize,
    },
    /// Identities are bound to sessions, but the runtime store a storage
    /// provider supplies holds no session at all: its backend lost its data,
    /// or the store does not outlive the process. Starting would bring every
    /// bound identity back on a lost session, where its turns fail, so the
    /// build refuses. Restore the backend's runtime data; or start over on a
    /// new, empty state directory (and a new mob store, if one is passed with
    /// `persistent_mob_storage()`): the identities then start from the
    /// roster on fresh sessions, and their custody and generation history
    /// starts over.
    RuntimeStoreLostSessions {
        state_dir: PathBuf,
        bound_identities: usize,
        provider: String,
    },
    /// The runtime store a storage provider supplies could not be read while
    /// the build checked it for the sessions identities are bound to.
    /// `detail` is the backend's error.
    RuntimeStoreUnreadable {
        state_dir: PathBuf,
        provider: String,
        detail: String,
    },
    /// A startup check could not read one of the state directory's stores.
    StartupProbeUnreadable {
        state_dir: PathBuf,
        file: PathBuf,
        detail: String,
    },
    /// The layout names a separate session database (a `store_path` with a
    /// file extension that is not an existing directory). Sessions live in
    /// `runtime.sqlite` under the state directory, so no separate session
    /// database is opened.
    SessionDatabaseOverrideRetired { path: PathBuf, state_dir: PathBuf },
    /// A persistent composition was given mob storage without saying whether
    /// it outlives the process: no provenance from
    /// [`crate::mob_composition_manifest::persistent_mob_storage`], and no
    /// [`crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral`].
    MobStorageUndeclared { state_dir: PathBuf },
}

impl Display for StorageLayoutError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileNameTwins { slot, paths } => {
                write!(
                    f,
                    "file-name twins for the {slot} store: multiple spellings exist in the same \
                     state directory ("
                )?;
                for (index, path) in paths.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", path.display())?;
                }
                write!(
                    f,
                    "); refusing to pick one at open — run the storage doctor \
                     (meta/storage/doctor) to inspect, and converge spellings with the \
                     storage migrate verb"
                )
            }
            Self::GatewayHomeUnavailable => {
                write!(
                    f,
                    "cannot derive the gateway home: neither XDG_STATE_HOME nor HOME is set"
                )
            }
            Self::PredecessorState { state_dir, paths } => {
                write!(
                    f,
                    "the state directory {} holds predecessor (pre-0.9) state (",
                    state_dir.display()
                )?;
                for (index, path) in paths.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", path.display())?;
                }
                // The directory is one shell word, quoted when it needs to be,
                // so the copied command works for a path with spaces or shell
                // metacharacters.
                write!(
                    f,
                    "); refusing to open renamed stores beside it - stop every process using \
                     this installation, then carry it over once with `meta_gateway \
                     storage-adopt-predecessor --state-dir {} --apply`",
                    crate::mobpack::shell_quote(&state_dir.to_string_lossy())
                )
            }
            Self::PredecessorGatewayHome { home, predecessor } => write!(
                f,
                "the predecessor (pre-0.9) gateway home {} still exists beside the gateway \
                 home {}; refusing to mint or pick a peer identity - carry it over once with \
                 `meta_gateway storage-adopt-predecessor --state-dir <dir> \
                 --predecessor-home {} --gateway-home {} --apply`, where <dir> is this \
                 installation's original state directory (the `persistent_state` of \
                 `rpc_gateway` and the SDK builders, or the store directory of `meta_gateway`)",
                predecessor.display(),
                home.display(),
                predecessor.display(),
                home.display()
            ),
            Self::RealmTwin { paths } => {
                write!(f, "two meerkat-level realm directories exist (")?;
                for (index, path) in paths.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", path.display())?;
                }
                write!(
                    f,
                    "); refusing to pick one - keep the realm that holds this installation's \
                     jobs and move the other aside"
                )
            }
            Self::AdoptedRealmInvalid {
                journal,
                value,
                reason,
            } => write!(
                f,
                "the adopted installation's recorded meerkat-level realm id ({}) in {} is \
                 invalid: {reason}; refusing rather than defaulting",
                value.as_deref().unwrap_or("missing"),
                journal.display()
            ),
            Self::AdoptionJournalUnreadable { path, detail } => write!(
                f,
                "the predecessor adoption journal {} cannot be read ({detail}); refusing \
                 rather than guessing which steps finished. Recover by hand: if the staged \
                 {}.next beside it decodes, move it over the journal; otherwise inspect the \
                 state directory and gateway home, write a journal that records only the \
                 steps that demonstrably finished (status in_progress), and rerun \
                 `meta_gateway storage-adopt-predecessor --state-dir <dir> --apply`, where \
                 <dir> is this installation's original state directory (the `persistent_state` \
                 of `rpc_gateway` and the SDK builders, or the store directory of \
                 `meta_gateway`), not necessarily the directory holding the journal",
                path.display(),
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("meta-adoption.json")
            ),
            Self::AdoptionIncomplete { path, detail } => write!(
                f,
                "the predecessor adoption at {} is unfinished ({detail}); rerun \
                 `meta_gateway storage-adopt-predecessor --state-dir <dir> --apply` (with the \
                 same home arguments) to resume it, where <dir> is this installation's \
                 original state directory (the `persistent_state` of `rpc_gateway` and the SDK \
                 builders, or the store directory of `meta_gateway`), not necessarily the \
                 directory holding the path above",
                path.display()
            ),
            Self::StateDirInUse { state_dir } => write!(
                f,
                "state directory {} is in use by another Meta runtime",
                state_dir.display()
            ),
            Self::StateDirUnavailable { state_dir, detail } => write!(
                f,
                "cannot claim state directory {}: {detail}",
                state_dir.display()
            ),
            Self::RuntimeInUse { state_dir, file } => write!(
                f,
                "state directory {} is in use: another process still hosts runtimes there \
                 ({} is locked); stop it before starting this runtime",
                state_dir.display(),
                file.display()
            ),
            Self::RuntimeLockUnreadable {
                state_dir,
                file,
                detail,
            } => write!(
                f,
                "cannot check whether another process hosts runtimes in state directory {} \
                 ({}): {detail}",
                state_dir.display(),
                file.display()
            ),
            Self::NativeStorageMigrationRequired { state_dir, status } => write!(
                f,
                "state directory {dir} must be migrated before this version opens it: {status}. \
                 Stop every process that uses it, run `{NATIVE_STORAGE_MIGRATION_VERB} \
                 --state-dir {dir}` to see the plan, then run it again with `--apply` \
                 (see {NATIVE_STORAGE_MIGRATION_GUIDE})",
                dir = state_dir.display()
            ),
            Self::RuntimeStoreMissing {
                state_dir,
                bound_identities,
            } => write!(
                f,
                "state directory {} has {bound_identities} identity binding(s) that name a \
                 session, but its runtime.sqlite is missing or holds no session; restore \
                 runtime.sqlite together with its -wal file from the backup taken with the rest \
                 of the directory",
                state_dir.display()
            ),
            Self::RuntimeStoreLostSessions {
                state_dir,
                bound_identities,
                provider,
            } => write!(
                f,
                "state directory {} has {bound_identities} identity binding(s) that name a \
                 session, but the runtime store of storage provider {provider} holds no \
                 session: its backend lost its data, or the store does not outlive the \
                 process. Restore the backend's runtime data; or start over on a new, empty \
                 state directory (and a new mob store, if you pass one with \
                 persistent_mob_storage()): the identities then start from the roster on fresh \
                 sessions, and their custody and generation history starts over",
                state_dir.display()
            ),
            Self::RuntimeStoreUnreadable {
                state_dir,
                provider,
                detail,
            } => write!(
                f,
                "the runtime store of storage provider {provider} could not be read to check \
                 the sessions of the identities bound in state directory {}: {detail}; make \
                 the backend readable, then start again",
                state_dir.display()
            ),
            Self::StartupProbeUnreadable {
                state_dir,
                file,
                detail,
            } => write!(
                f,
                "cannot check state directory {} before startup ({}): {detail}",
                state_dir.display(),
                file.display()
            ),
            Self::SessionDatabaseOverrideRetired { path, state_dir } => write!(
                f,
                "store_path names the session database {path}, but this version keeps sessions \
                 in runtime.sqlite under the state directory {dir} and opens no separate session \
                 database. If {path} holds sessions, first run `{SESSION_DATABASE_IMPORT_VERB} \
                 --state-dir {dir} --session-db {path} --apply` (it moves the file into the \
                 layout), then `{NATIVE_STORAGE_MIGRATION_VERB} --state-dir {dir}` (a dry run; \
                 run it again with `--apply`). Then pass the state directory {dir} as \
                 store_path",
                path = path.display(),
                dir = state_dir.display()
            ),
            Self::MobStorageUndeclared { state_dir } => write!(
                f,
                "the persistent composition over state directory {} was given mob storage \
                 without a provenance: pass the pair that persistent_mob_storage(path) returns \
                 for durable mob storage, or MobStorageProvenance::declared_ephemeral() for \
                 in-memory mob storage",
                state_dir.display()
            ),
        }
    }
}

impl std::error::Error for StorageLayoutError {}

/// Immutable path authority, constructed once at bootstrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaStorageLayout {
    state_dir: PathBuf,
    gateway_home: Option<PathBuf>,
    session_db_override: Option<PathBuf>,
    durability: StateDirDurability,
    meerkat_state_root: Option<PathBuf>,
    /// The installation's meerkat-level realm id, or why the installation
    /// must refuse. Resolved once, at construction
    /// ([`crate::storage_predecessor::resolve_installation`]).
    installation: Result<meerkat_core::RealmId, StorageLayoutError>,
}

impl MetaStorageLayout {
    /// Embedded construction: Meta runs inside a Meerkat realm and derives
    /// from Meerkat's already-resolved [`meerkat_core::StorageLayout`] —
    /// Meta never re-resolves ambient roots. `state_dir` is the directory
    /// Meta's stores live in (the embedder chooses where under the realm);
    /// the Meerkat state root is carried for the layout summary so health
    /// surfaces can correlate the two.
    pub fn from_meerkat_layout(
        meerkat_layout: &meerkat_core::StorageLayout,
        state_dir: PathBuf,
    ) -> Self {
        Self {
            installation: crate::storage_predecessor::resolve_installation(&state_dir),
            state_dir,
            gateway_home: None,
            session_db_override: None,
            durability: StateDirDurability::Durable,
            meerkat_state_root: Some(meerkat_layout.state_root().to_path_buf()),
        }
    }

    /// Standalone gateway construction from an explicit state directory plus
    /// the gateway home (registry + peer key). Derive the home with
    /// [`default_gateway_home`]; nothing else may read `$XDG_STATE_HOME`.
    pub fn standalone(state_dir: PathBuf, gateway_home: PathBuf) -> Self {
        Self {
            installation: crate::storage_predecessor::resolve_installation(&state_dir),
            state_dir,
            gateway_home: Some(gateway_home),
            session_db_override: None,
            durability: StateDirDurability::Durable,
            meerkat_state_root: None,
        }
    }

    /// Standalone construction from a gateway `store_path` init parameter:
    /// an existing directory is the state directory whatever its name
    /// (`/srv/meta.d`); otherwise a path with a file extension is an explicit
    /// session-database override (its parent becomes the state directory),
    /// and any other path is the state directory itself. This is the one
    /// place that interprets the `store_path` escape hatch - call sites never
    /// sniff extensions.
    pub fn standalone_from_store_path(store_path: &Path, gateway_home: PathBuf) -> Self {
        if store_path.extension().is_some() && !store_path.is_dir() {
            let state_dir = store_path
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
            Self {
                installation: crate::storage_predecessor::resolve_installation(&state_dir),
                state_dir,
                gateway_home: Some(gateway_home),
                session_db_override: Some(store_path.to_path_buf()),
                durability: StateDirDurability::Durable,
                meerkat_state_root: None,
            }
        } else {
            Self::standalone(store_path.to_path_buf(), gateway_home)
        }
    }

    /// Explicitly declared-ephemeral construction: no persistent state
    /// exists, and every store under `scratch_root` is per-process scratch.
    /// The choice is recorded in the layout summary — never a silent
    /// call-site fallback into the OS temp directory.
    pub fn declared_ephemeral(scratch_root: PathBuf) -> Self {
        Self {
            // A scratch root is usually fresh, but a realm an earlier start
            // explicitly established in it is recognized like any other.
            installation: crate::storage_predecessor::resolve_installation(&scratch_root),
            state_dir: scratch_root,
            gateway_home: None,
            session_db_override: None,
            durability: StateDirDurability::DeclaredEphemeral,
            meerkat_state_root: None,
        }
    }

    /// Fully-injected constructor: no ambient reads at all (tests, and
    /// embedders that own their environment — the runtime builder constructs
    /// its layout from the explicit `persistent_state()` root this way).
    pub fn with_injected_roots(state_dir: PathBuf, gateway_home: Option<PathBuf>) -> Self {
        Self {
            installation: crate::storage_predecessor::resolve_installation(&state_dir),
            state_dir,
            gateway_home,
            session_db_override: None,
            durability: StateDirDurability::Durable,
            meerkat_state_root: None,
        }
    }

    /// The state directory every database locator resolves under.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// The installation's meerkat-level realm id: the Meta default for a
    /// fresh installation, the value recorded by predecessor adoption for an
    /// adopted one. Refuses typed (never a default) when the installation is
    /// unadopted, mid-adoption, or carries an invalid recorded value.
    pub fn meerkat_level_realm(&self) -> Result<&meerkat_core::RealmId, StorageLayoutError> {
        self.installation.as_ref().map_err(Clone::clone)
    }

    /// The XDG gateway home (registry + peer key), when this layout has one.
    pub fn gateway_home(&self) -> Option<&Path> {
        self.gateway_home.as_deref()
    }

    /// The Meerkat realm state root, when embedded via
    /// [`Self::from_meerkat_layout`].
    pub fn meerkat_state_root(&self) -> Option<&Path> {
        self.meerkat_state_root.as_deref()
    }

    /// The explicit session-database override, when constructed from a
    /// `store_path` with a file extension.
    pub fn session_db_override(&self) -> Option<&Path> {
        self.session_db_override.as_deref()
    }

    /// Whether the state root is an explicitly declared scratch root.
    pub fn is_declared_ephemeral(&self) -> bool {
        self.durability == StateDirDurability::DeclaredEphemeral
    }

    /// Resolve a database locator by canonical-name-first probing (see the
    /// module docs). The resolver never creates files and never renames.
    pub fn resolve_database(
        &self,
        slot: DatabaseSlot,
    ) -> Result<ResolvedDatabase, StorageLayoutError> {
        // The installation refusal comes first: an explicit session file
        // inside an unadopted, mid-adoption or unresolvable installation
        // must not open before the rest of startup refuses.
        if let Err(refusal) = &self.installation {
            return Err(refusal.clone());
        }
        if slot == DatabaseSlot::Sessions
            && let Some(override_path) = self.session_db_override.as_ref()
        {
            return Ok(ResolvedDatabase {
                path: override_path.clone(),
                provenance: DatabaseProvenance::ExplicitOverride,
            });
        }
        let canonical = self.state_dir.join(slot.canonical_name());
        let existing_legacy: Vec<(&'static str, PathBuf)> = slot
            .legacy_names()
            .iter()
            .map(|name| (*name, self.state_dir.join(name)))
            .filter(|(_, path)| path.exists())
            .collect();
        let canonical_exists = canonical.exists();
        if (canonical_exists && !existing_legacy.is_empty()) || existing_legacy.len() > 1 {
            let mut paths = Vec::with_capacity(existing_legacy.len() + 1);
            if canonical_exists {
                paths.push(canonical);
            }
            paths.extend(existing_legacy.into_iter().map(|(_, path)| path));
            return Err(StorageLayoutError::FileNameTwins { slot, paths });
        }
        if let Some((name, path)) = existing_legacy.into_iter().next() {
            return Ok(ResolvedDatabase {
                path,
                provenance: DatabaseProvenance::LegacySpelling(name.to_string()),
            });
        }
        Ok(ResolvedDatabase {
            path: canonical,
            provenance: DatabaseProvenance::Canonical,
        })
    }

    /// The sessions database (probing; honors the explicit override).
    pub fn session_db(&self) -> Result<ResolvedDatabase, StorageLayoutError> {
        self.resolve_database(DatabaseSlot::Sessions)
    }

    /// The identity-continuity database (probing).
    pub fn continuity_db(&self) -> Result<ResolvedDatabase, StorageLayoutError> {
        self.resolve_database(DatabaseSlot::Continuity)
    }

    /// The runtime-metadata database (probing).
    pub fn metadata_db(&self) -> Result<ResolvedDatabase, StorageLayoutError> {
        self.resolve_database(DatabaseSlot::Metadata)
    }

    /// The console-timeline database (probing).
    pub fn console_db(&self) -> Result<ResolvedDatabase, StorageLayoutError> {
        self.resolve_database(DatabaseSlot::Console)
    }

    /// The agent-memory root directory (probing; per-realm files beneath it
    /// stay feature-owned).
    pub fn agent_memory_root(&self) -> Result<ResolvedDatabase, StorageLayoutError> {
        self.resolve_database(DatabaseSlot::AgentMemory)
    }

    /// The runtime store database. One spelling everywhere — infallible.
    pub fn runtime_db(&self) -> PathBuf {
        self.state_dir.join(RUNTIME_DB_FILE_NAME)
    }

    /// The schedule store database. One spelling everywhere — infallible.
    pub fn schedule_db(&self) -> PathBuf {
        self.state_dir.join(SCHEDULE_STORE_FILE)
    }

    /// The workgraph store database. One spelling everywhere — infallible.
    pub fn workgraph_db(&self) -> PathBuf {
        self.state_dir.join(WORKGRAPH_STORE_FILE)
    }

    /// The temporary-council store database.
    ///
    /// Meta owns this path rather than letting meerkat's
    /// `with_persistent_storage_root` pick one, so the council store enters
    /// Meta's own slot authority (doctor, migrate, health) instead of
    /// existing as a durable file nothing here enumerates.
    pub fn council_db(&self) -> PathBuf {
        self.state_dir.join(COUNCIL_STORE_FILE)
    }

    /// The directory durable agent-created child mobs live in (see
    /// [`crate::ChildMobStorage`]). Meerkat owns the name; this is derived
    /// from its custody path rather than spelled again here.
    pub fn child_mob_root(&self) -> PathBuf {
        crate::child_mob_storage::child_mob_root(&self.state_dir)
    }

    #[cfg(feature = "extension-state")]
    pub fn extension_state_db(&self) -> Result<ResolvedDatabase, StorageLayoutError> {
        self.resolve_database(DatabaseSlot::ExtensionState)
    }

    /// The mob announcement database
    /// ([`MOB_ANNOUNCEMENTS_DB_FILE_NAME`] in the state directory). Refused,
    /// like every slot, while the installation is refused.
    pub fn mob_announcements_db(&self) -> Result<PathBuf, StorageLayoutError> {
        if let Err(refusal) = &self.installation {
            return Err(refusal.clone());
        }
        Ok(self.state_dir.join(MOB_ANNOUNCEMENTS_DB_FILE_NAME))
    }

    /// Canonical Meerkat-level detached-job database inherited by Meta's
    /// composite provider. This is realm-owned, not a second Meta store.
    pub fn jobs_db(&self) -> Result<PathBuf, StorageLayoutError> {
        let realm = self.meerkat_level_realm()?;
        Ok(meerkat_store::realm_paths_in(&self.state_dir, realm.as_str()).jobs_sqlite_path)
    }

    /// The reserved event-log locator (nothing opens it before M4).
    pub fn event_log_db(&self) -> PathBuf {
        self.state_dir.join(EVENT_LOG_DB_FILE_NAME)
    }

    /// The blob root directory (internal sharding stays feature-owned).
    pub fn blob_root(&self) -> PathBuf {
        self.state_dir.join(BLOB_ROOT_DIR_NAME)
    }

    /// The gateway peer-key file, when this layout has a gateway home.
    pub fn peer_key_file(&self) -> Option<PathBuf> {
        self.gateway_home
            .as_ref()
            .map(|home| home.join(KEY_FILE_NAME))
    }

    /// The runtime registry file, when this layout has a gateway home.
    pub fn registry_file(&self) -> Option<PathBuf> {
        self.gateway_home
            .as_ref()
            .map(|home| home.join(RUNTIME_REGISTRY_FILE_NAME))
    }

    /// A serializable snapshot of the layout and every slot's resolution,
    /// for health surfaces (the storage doctor consumes it).
    pub fn layout_summary(&self) -> StorageLayoutSummary {
        let databases = DatabaseSlot::ALL
            .into_iter()
            .map(|slot| {
                let resolution = match self.resolve_database(slot) {
                    Ok(resolved) => DatabaseResolution::Resolved {
                        path: resolved.path,
                        provenance: resolved.provenance,
                    },
                    Err(StorageLayoutError::FileNameTwins { paths, .. }) => {
                        DatabaseResolution::Twins { paths }
                    }
                    Err(StorageLayoutError::PredecessorState { paths, .. }) => {
                        DatabaseResolution::PredecessorState { paths }
                    }
                    Err(
                        StorageLayoutError::AdoptionIncomplete { path, .. }
                        | StorageLayoutError::AdoptionJournalUnreadable { path, .. },
                    ) => DatabaseResolution::AdoptionIncomplete { journal: path },
                    Err(
                        error @ (StorageLayoutError::RealmTwin { .. }
                        | StorageLayoutError::AdoptedRealmInvalid { .. }),
                    ) => DatabaseResolution::RealmRefused {
                        detail: error.to_string(),
                    },
                    // Slot resolution fails only on twins or installation state.
                    Err(_) => {
                        unreachable!("resolve_database fails only on twins or installation state")
                    }
                };
                DatabaseSummary { slot, resolution }
            })
            .collect();
        StorageLayoutSummary {
            state_dir: self.state_dir.clone(),
            durability: self.durability,
            gateway_home: self.gateway_home.clone(),
            meerkat_state_root: self.meerkat_state_root.clone(),
            blob_root: self.blob_root(),
            peer_key_file: self.peer_key_file(),
            registry_file: self.registry_file(),
            databases,
        }
    }
}

/// Per-slot entry in the layout summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseSummary {
    pub slot: DatabaseSlot,
    pub resolution: DatabaseResolution,
}

/// How a slot stands on disk at summary time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum DatabaseResolution {
    Resolved {
        path: PathBuf,
        provenance: DatabaseProvenance,
    },
    Twins {
        paths: Vec<PathBuf>,
    },
    /// Predecessor (pre-0.9) state blocks every slot until adopted.
    PredecessorState {
        paths: Vec<PathBuf>,
    },
    /// An unfinished predecessor adoption blocks every slot until resumed.
    AdoptionIncomplete {
        journal: PathBuf,
    },
    /// The installation's meerkat-level realm cannot be resolved (twin
    /// realm directories, or an invalid recorded value).
    RealmRefused {
        detail: String,
    },
}

/// Serializable layout snapshot for health surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageLayoutSummary {
    pub state_dir: PathBuf,
    pub durability: StateDirDurability,
    pub gateway_home: Option<PathBuf>,
    pub meerkat_state_root: Option<PathBuf>,
    pub blob_root: PathBuf,
    pub peer_key_file: Option<PathBuf>,
    pub registry_file: Option<PathBuf>,
    pub databases: Vec<DatabaseSummary>,
}

/// The sanctioned ambient derivation of the gateway home:
/// `$XDG_STATE_HOME/meerkat-meta` when `XDG_STATE_HOME` is set and
/// non-blank, else `$HOME/.local/state/meerkat-meta`.
///
/// Refuses with [`StorageLayoutError::PredecessorGatewayHome`] while the
/// predecessor sibling exists, and with
/// [`StorageLayoutError::AdoptionIncomplete`] while the home still carries
/// a predecessor name (see the module docs).
pub fn default_gateway_home() -> Result<PathBuf, StorageLayoutError> {
    refuse_unadopted_predecessor_home(derived_gateway_home()?)
}

/// The ambient gateway home derivation without the predecessor refusal:
/// the target the predecessor adoption verb moves the invoking user's own
/// predecessor home to, when asked (`--user-gateway-home`).
pub fn derived_gateway_home() -> Result<PathBuf, StorageLayoutError> {
    gateway_home_from(
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// Pass `home` through unless its predecessor sibling still exists, or the
/// home still carries a predecessor name (an unfinished adoption).
pub fn refuse_unadopted_predecessor_home(home: PathBuf) -> Result<PathBuf, StorageLayoutError> {
    if let Some(predecessor) = crate::storage_predecessor::predecessor_gateway_home_beside(&home)
        && predecessor.exists()
    {
        return Err(StorageLayoutError::PredecessorGatewayHome { home, predecessor });
    }
    let predecessor_drafts =
        home.join(crate::storage_predecessor::PREDECESSOR_DRAFT_STORE_FILE_NAME);
    if predecessor_drafts.exists() {
        return Err(StorageLayoutError::AdoptionIncomplete {
            path: predecessor_drafts,
            detail: "the gateway home still carries the predecessor draft store".into(),
        });
    }
    Ok(home)
}

fn gateway_home_from(
    xdg_state_home: Option<&str>,
    home: Option<&str>,
) -> Result<PathBuf, StorageLayoutError> {
    if let Some(xdg) = xdg_state_home
        && !xdg.trim().is_empty()
    {
        return Ok(PathBuf::from(xdg).join(GATEWAY_HOME_DIR_NAME));
    }
    let home = home.ok_or(StorageLayoutError::GatewayHomeUnavailable)?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("state")
        .join(GATEWAY_HOME_DIR_NAME))
}

/// Resolve a supplied storage root or file override to its physical path.
///
/// Existing paths use filesystem canonicalization. For an absent path, resolve
/// its deepest existing ancestor and append the missing components without
/// creating them. This keeps the storage identity stable when its directories
/// or database file are first created. Relative paths anchor at the current
/// working directory, exactly where the store would create them.
///
/// Filesystem errors other than a missing path are returned unchanged. A
/// parent traversal above a missing component is refused because its physical
/// identity cannot be established by removing path components textually.
pub fn canonicalize_storage_root(path: &Path) -> std::io::Result<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut missing_tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if existing.as_os_str().is_empty() {
            existing = std::env::current_dir()?;
            continue;
        }
        match std::fs::canonicalize(&existing) {
            Ok(mut canonical) => {
                for component in missing_tail.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name().map(std::ffi::OsStr::to_os_string) else {
                    // A `..` or root ending above a missing component cannot
                    // be resolved textually without lying about identity.
                    return Err(error);
                };
                missing_tail.push(name);
                existing = existing.parent().map(PathBuf::from).unwrap_or_default();
            }
            Err(error) => return Err(error),
        }
    }
}

/// The sanctioned per-process scratch root for
/// [`MetaStorageLayout::declared_ephemeral`] layouts: a pid-suffixed
/// directory under the OS temp directory (per-process, so two gateways on
/// one host never share scratch identity state).
pub fn default_ephemeral_scratch_root() -> PathBuf {
    std::env::temp_dir().join(format!("meta-scratch-{}", std::process::id()))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::write(path, b"").expect("touch fixture file");
    }

    fn durable_layout(dir: &Path) -> MetaStorageLayout {
        MetaStorageLayout::with_injected_roots(dir.to_path_buf(), None)
    }

    #[cfg(unix)]
    #[test]
    fn canonical_storage_root_is_stable_before_and_after_nested_directory_creation() {
        let temp = tempfile::tempdir().expect("scratch directory");
        let physical = temp.path().join("physical");
        std::fs::create_dir(&physical).expect("physical parent");
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&physical, &alias).expect("symlinked parent");
        let supplied = alias.join("not-created/nested/state");
        let expected = physical
            .canonicalize()
            .expect("canonical physical parent")
            .join("not-created/nested/state");

        let before = canonicalize_storage_root(&supplied).expect("resolve absent nested root");
        assert_eq!(before, expected);
        assert!(
            !supplied.exists(),
            "path resolution must not create storage"
        );
        std::fs::create_dir_all(&supplied).expect("create storage after resolution");
        assert_eq!(
            canonicalize_storage_root(&supplied).expect("resolve existing root"),
            before,
            "creating the same physical root must not change its identity"
        );
    }

    #[cfg(unix)]
    #[test]
    fn canonical_storage_root_preserves_an_absent_session_file_override() {
        let temp = tempfile::tempdir().expect("scratch directory");
        let physical = temp.path().join("physical");
        std::fs::create_dir(&physical).expect("physical parent");
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&physical, &alias).expect("symlinked parent");
        let supplied = alias.join("not-created/sessions.sqlite3");
        let before = canonicalize_storage_root(&supplied).expect("resolve absent file override");
        assert_eq!(
            before,
            physical
                .canonicalize()
                .expect("canonical physical parent")
                .join("not-created/sessions.sqlite3")
        );
        assert!(!supplied.exists(), "resolving a DB path must not create it");
        std::fs::create_dir_all(supplied.parent().expect("DB parent")).expect("create DB parent");
        std::fs::write(&supplied, b"").expect("create DB override");
        assert_eq!(
            canonicalize_storage_root(&supplied).expect("existing DB override"),
            before
        );
        let layout =
            MetaStorageLayout::standalone_from_store_path(&before, temp.path().join("gateway"));
        assert_eq!(
            layout.session_db().expect("resolve session override").path,
            before
        );
    }

    #[test]
    fn canonical_storage_root_refuses_parent_traversal_above_a_missing_component() {
        let temp = tempfile::tempdir().expect("scratch directory");
        let supplied = temp.path().join("not-created/../state");
        let error =
            canonicalize_storage_root(&supplied).expect_err("unresolved parent is ambiguous");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(!temp.path().join("not-created").exists());
        assert!(!temp.path().join("state").exists());
    }

    #[test]
    fn canonical_storage_root_preserves_non_missing_filesystem_errors() {
        let temp = tempfile::tempdir().expect("scratch directory");
        let file = temp.path().join("file");
        std::fs::write(&file, b"not a directory").expect("create regular file");
        let supplied = file.join("state");
        let actual = canonicalize_storage_root(&supplied).expect_err("file cannot contain a root");
        let direct = supplied
            .canonicalize()
            .expect_err("filesystem rejects file parent");
        assert_eq!(actual.kind(), direct.kind());
        assert_eq!(actual.raw_os_error(), direct.raw_os_error());
    }

    #[test]
    fn fresh_dir_resolves_every_slot_to_canonical() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let layout = durable_layout(tmp.path());
        for slot in DatabaseSlot::ALL {
            let resolved = layout.resolve_database(slot).expect("fresh dir resolves");
            assert_eq!(resolved.provenance, DatabaseProvenance::Canonical);
            assert_eq!(resolved.path, tmp.path().join(slot.canonical_name()));
        }
        assert_eq!(
            layout.session_db().expect("sessions").path,
            tmp.path().join("sessions.sqlite3")
        );
        assert_eq!(
            layout.continuity_db().expect("continuity").path,
            tmp.path().join("continuity.sqlite3")
        );
    }

    #[test]
    fn canonical_only_resolves_canonical() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch(&tmp.path().join("sessions.sqlite3"));
        let resolved = durable_layout(tmp.path()).session_db().expect("resolve");
        assert_eq!(resolved.provenance, DatabaseProvenance::Canonical);
        assert_eq!(resolved.path, tmp.path().join("sessions.sqlite3"));
    }

    #[test]
    fn legacy_only_resolves_where_it_lies() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let layout = durable_layout(tmp.path());
        for (slot, legacy) in [
            (DatabaseSlot::Sessions, "sessions.db"),
            (DatabaseSlot::Sessions, "sessions.sqlite"),
            (DatabaseSlot::Continuity, "continuity.db"),
            (DatabaseSlot::Continuity, "identity_continuity.sqlite"),
        ] {
            let path = tmp.path().join(legacy);
            touch(&path);
            let resolved = layout.resolve_database(slot).expect("legacy resolves");
            assert_eq!(
                resolved.provenance,
                DatabaseProvenance::LegacySpelling(legacy.to_string()),
                "slot {slot} legacy {legacy}"
            );
            assert_eq!(resolved.path, path);
            std::fs::remove_file(&path).expect("cleanup fixture");
        }
        // The agent-memory slot probes a directory, not a file.
        let legacy_dir = tmp.path().join("agent-memory-sqlite");
        std::fs::create_dir(&legacy_dir).expect("legacy memory dir");
        let resolved = layout.agent_memory_root().expect("legacy dir resolves");
        assert_eq!(
            resolved.provenance,
            DatabaseProvenance::LegacySpelling("agent-memory-sqlite".to_string())
        );
        assert_eq!(resolved.path, legacy_dir);
    }

    #[test]
    fn canonical_plus_legacy_refuses_as_twins() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch(&tmp.path().join("sessions.sqlite3"));
        touch(&tmp.path().join("sessions.db"));
        let err = durable_layout(tmp.path())
            .session_db()
            .expect_err("twins refuse");
        let StorageLayoutError::FileNameTwins { slot, paths } = err else {
            panic!("expected FileNameTwins");
        };
        assert_eq!(slot, DatabaseSlot::Sessions);
        assert_eq!(
            paths,
            vec![
                tmp.path().join("sessions.sqlite3"),
                tmp.path().join("sessions.db"),
            ]
        );
    }

    #[test]
    fn two_legacy_spellings_refuse_as_twins() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch(&tmp.path().join("continuity.db"));
        touch(&tmp.path().join("identity_continuity.sqlite"));
        let err = durable_layout(tmp.path())
            .continuity_db()
            .expect_err("legacy twins refuse");
        let StorageLayoutError::FileNameTwins { slot, paths } = err else {
            panic!("expected FileNameTwins");
        };
        assert_eq!(slot, DatabaseSlot::Continuity);
        assert_eq!(paths.len(), 2);
        // The message points operators at the doctor.
        assert!(err_to_string(&slot, &paths).contains("meta/storage/doctor"));
    }

    fn err_to_string(slot: &DatabaseSlot, paths: &[PathBuf]) -> String {
        StorageLayoutError::FileNameTwins {
            slot: *slot,
            paths: paths.to_vec(),
        }
        .to_string()
    }

    #[test]
    fn store_path_with_extension_is_an_explicit_session_override() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_file = tmp.path().join("custom-sessions.db");
        let layout = MetaStorageLayout::standalone_from_store_path(
            &db_file,
            tmp.path().join("gateway-home"),
        );
        assert_eq!(layout.state_dir(), tmp.path());
        assert_eq!(layout.session_db_override(), Some(db_file.as_path()));
        // The override bypasses probing entirely — even a twin pair on disk
        // does not refuse an explicitly chosen file.
        touch(&tmp.path().join("sessions.sqlite3"));
        touch(&tmp.path().join("sessions.db"));
        let resolved = layout.session_db().expect("override resolves");
        assert_eq!(resolved.provenance, DatabaseProvenance::ExplicitOverride);
        assert_eq!(resolved.path, db_file);
        // Other slots still probe normally.
        assert!(layout.continuity_db().is_ok());
    }

    #[test]
    fn store_path_without_extension_is_the_state_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("state");
        let layout = MetaStorageLayout::standalone_from_store_path(&dir, tmp.path().join("home"));
        assert_eq!(layout.state_dir(), dir);
        assert_eq!(layout.session_db_override(), None);
    }

    #[test]
    fn gateway_home_prefers_non_blank_xdg_state_home() {
        assert_eq!(
            gateway_home_from(Some("/xdg/state"), Some("/home/user")).expect("xdg"),
            PathBuf::from("/xdg/state").join("meerkat-meta")
        );
        assert_eq!(
            gateway_home_from(Some("   "), Some("/home/user")).expect("blank xdg falls back"),
            PathBuf::from("/home/user")
                .join(".local")
                .join("state")
                .join("meerkat-meta")
        );
        assert_eq!(
            gateway_home_from(None, Some("/home/user")).expect("home fallback"),
            PathBuf::from("/home/user")
                .join(".local")
                .join("state")
                .join("meerkat-meta")
        );
        assert_eq!(
            gateway_home_from(None, None).expect_err("no roots"),
            StorageLayoutError::GatewayHomeUnavailable
        );
    }

    #[test]
    fn embedded_layout_has_no_gateway_home_and_records_the_meerkat_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let meerkat = meerkat_core::StorageLayout::with_injected_roots(
            tmp.path().to_path_buf(),
            None,
            None,
            tmp.path().join("realm-root"),
        );
        let layout =
            MetaStorageLayout::from_meerkat_layout(&meerkat, tmp.path().join("meta-state"));
        assert_eq!(layout.state_dir(), tmp.path().join("meta-state"));
        assert_eq!(
            layout.meerkat_state_root(),
            Some(tmp.path().join("realm-root").as_path())
        );
        assert_eq!(layout.gateway_home(), None);
        assert_eq!(layout.peer_key_file(), None);
        assert_eq!(layout.registry_file(), None);
        assert!(!layout.is_declared_ephemeral());
    }

    #[test]
    fn standalone_layout_owns_the_gateway_home_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("gw-home");
        let layout = MetaStorageLayout::standalone(tmp.path().join("state"), home.clone());
        assert_eq!(layout.gateway_home(), Some(home.as_path()));
        assert_eq!(layout.peer_key_file(), Some(home.join("peer_key.ed25519")));
        assert_eq!(layout.registry_file(), Some(home.join("tux-runtimes.json")));
    }

    #[test]
    fn declared_ephemeral_layout_is_recorded_in_the_summary() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let layout = MetaStorageLayout::declared_ephemeral(tmp.path().join("scratch"));
        assert!(layout.is_declared_ephemeral());
        let summary = layout.layout_summary();
        assert_eq!(summary.durability, StateDirDurability::DeclaredEphemeral);
        assert_eq!(summary.state_dir, tmp.path().join("scratch"));
    }

    #[test]
    fn layout_summary_round_trips_through_serde_and_reports_twins() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch(&tmp.path().join("sessions.sqlite3"));
        touch(&tmp.path().join("sessions.db"));
        touch(&tmp.path().join("continuity.db"));
        let layout =
            MetaStorageLayout::standalone(tmp.path().to_path_buf(), tmp.path().join("home"));
        let summary = layout.layout_summary();
        let sessions = summary
            .databases
            .iter()
            .find(|entry| entry.slot == DatabaseSlot::Sessions)
            .expect("sessions entry");
        assert!(matches!(
            sessions.resolution,
            DatabaseResolution::Twins { ref paths } if paths.len() == 2
        ));
        let continuity = summary
            .databases
            .iter()
            .find(|entry| entry.slot == DatabaseSlot::Continuity)
            .expect("continuity entry");
        assert!(matches!(
            continuity.resolution,
            DatabaseResolution::Resolved {
                provenance: DatabaseProvenance::LegacySpelling(ref name),
                ..
            } if name == "continuity.db"
        ));
        let json = serde_json::to_string(&summary).expect("serialize summary");
        let restored: StorageLayoutSummary =
            serde_json::from_str(&json).expect("deserialize summary");
        assert_eq!(restored, summary);
    }

    #[test]
    fn fixed_name_slots_agree_with_the_shared_wiring_constants() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let layout = durable_layout(tmp.path());
        assert_eq!(layout.runtime_db(), tmp.path().join("runtime.sqlite"));
        assert_eq!(
            layout.schedule_db(),
            tmp.path().join(crate::schedule_wiring::SCHEDULE_STORE_FILE)
        );
        assert_eq!(
            layout.workgraph_db(),
            tmp.path()
                .join(crate::workgraph_wiring::WORKGRAPH_STORE_FILE)
        );
        assert_eq!(
            layout.jobs_db().expect("fresh installation resolves"),
            tmp.path()
                .join(crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID)
                .join("jobs.sqlite3")
        );
        assert_eq!(layout.blob_root(), tmp.path().join("blobs"));
        assert_eq!(layout.event_log_db(), tmp.path().join("event_log.sqlite3"));
    }
}

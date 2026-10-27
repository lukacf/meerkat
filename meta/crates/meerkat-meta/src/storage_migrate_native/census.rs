//! The read-only census of `meta_gateway storage-migrate-native`.
//!
//! It classifies every session of a 0.9.3 gateway state directory by
//! comparing the committed WholeBlob session in `runtime.sqlite` with the
//! copy in Meta's projection (`sessions.sqlite3`, or `continuity.sqlite3`
//! when a roster provider was registered). The comparison uses the 0.8.11
//! durable-head relation provers in `super::relation`, the same proofs the
//! 0.9.3 facade ran on every boundary.
//!
//! The census reads the startup predicate over the live directory
//! (read-only), refuses a directory a runtime is using, then reads scratch
//! copies taken under the maintenance fence; no store ever opens a live
//! database. A row is admitted only when the migrated directory keeps
//! everything a 0.9.3 gateway would still serve for that session.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use meerkat::SessionStore;
use meerkat_core::SessionLifecycleTerminal;
use meerkat_core::types::SessionId;
use meerkat_runtime::{LogicalRuntimeId, RuntimeState, RuntimeStore, SqliteRuntimeStore};
use meerkat_store::sqlite_store::SESSION_STORE_DOMAIN;
use serde::Serialize;

use super::apply::{
    self, ApplyJournal, ApplyPlan, DiscardedRuntime, NativeApplyOutcome, RetiredProjection,
    Retirement,
};
use super::export_census::ContinuityExportSummary;
use super::identity::{
    ClaimSource, FirstBootRowPlan, IdentityCensusRow, IdentityClaim, identity_census,
};
use super::import::{ImportSource, ProjectionStore};
use super::layout_status::{
    CONTINUITY_SCHEMA_DOMAIN, NativeLayoutStatus, UnrecognizedFile, is_retired_stamp_file,
    native_layout_status, unrecognized_session_databases,
};
use super::relation::{self, DurableHeadRelation};
use super::runtime_conversion_began;
use crate::identity_first::{ContinuitySessionStoreAdapter, LocalContinuityStore};
use crate::storage_layout::{
    CANONICAL_CONTINUITY_DB_FILE_NAME, CANONICAL_SESSIONS_DB_FILE_NAME, DatabaseSlot,
    EVENT_LOG_DB_FILE_NAME, RUNTIME_DB_FILE_NAME,
};
use crate::storage_migrate::{MetaMaintenanceFence, MigrateMode};

/// How long the fence waits (total, across all files) for in-flight
/// per-operation guards to drain before reporting a foreign holder.
pub(super) const FENCE_DRAIN_DEADLINE: Duration = Duration::from_secs(10);

/// The lock file a 0.9.4 Meta runtime holds for as long as it uses a state
/// directory.
pub(super) const META_RUNTIME_LOCK_FILE_NAME: &str = "meta-runtime.lock";

/// The name of the durable mob store's copy in the scratch directory.
pub(super) const MOB_STORE_SCRATCH_NAME: &str = "mob-store.sqlite3";

/// The lifetime lock Core's runtime store holds beside its database for as
/// long as an owner has it open (a 0.9.3 gateway included).
const RUNTIME_EXECUTION_CUSTODY_SUFFIX: &str = ".execution-custody";

/// The continuity tables that hold a session body: a head row, or a
/// snapshot row.
const CONTINUITY_SESSION_BODY_TABLES: [&str; 2] = ["continuity_session_heads", "session_snapshots"];

/// The tables of a `sessions.sqlite3` whose rows no release reads: the schema
/// bookkeeping, and the schedule tables an older layout kept there. Meta
/// reads schedules only from `schedule.sqlite` (since 0.9.0).
const INERT_SESSIONS_TABLES: [&str; 4] = [
    "meerkat_schema",
    "schedule_schedules",
    "schedule_occurrences",
    "schedule_receipts",
];

/// The 0.9.3 layouts the verb knows how to migrate: a gateway's, and an
/// application's that embeds the runtime builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GatewayStateShape {
    /// No roster provider: `sessions.sqlite3` is the session projection.
    SessionsProjection,
    /// A roster provider with durable mob storage: `continuity.sqlite3` is
    /// the projection and the mob store (`event_log.sqlite3`, or the store
    /// `--mob-store` names) holds the mob.
    ContinuityProjection,
    /// A roster provider over in-memory mob storage: `continuity.sqlite3`
    /// and no mob store, so no identity has a durable Core binding.
    ContinuityProjectionInMemoryMob,
    /// An application that embeds the runtime builder with a roster
    /// provider: `sessions.sqlite3` is the session projection and
    /// `continuity.sqlite3` holds only the identities' records, no session
    /// body. [`NativeMigrateReport::mob_storage`] says which identity rules
    /// apply.
    SessionsProjectionWithBindings,
    /// A continuity export (`--continuity-export`) into a new state
    /// directory: `--apply` creates the native pair and imports every ready
    /// entry.
    ContinuityExportNew,
    /// A continuity export into the host's own state directory, which holds
    /// its `runtime.sqlite` and no local projection: the export is the
    /// projection side.
    ContinuityExportInPlace,
}

/// The mob storage the identity census reads for the directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CensusMobStore {
    /// No durable mob store: the mob lived in memory.
    InMemory,
    /// The durable mob store at `path`: `event_log.sqlite3` in the
    /// directory, or the store `--mob-store` names.
    Durable { path: PathBuf },
}

impl std::fmt::Display for CensusMobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InMemory => f.write_str("in memory"),
            Self::Durable { path } => write!(f, "{}", path.display()),
        }
    }
}

/// How one session's committed runtime copy relates to its projection copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionCensusClass {
    /// Both copies hold the same transcript.
    Equal,
    /// The projection is a proven prefix of the committed session: it lags,
    /// and `runtime.sqlite` already holds everything.
    ProjectionBehind,
    /// Only `runtime.sqlite` holds the session.
    ProjectionMissing,
    /// Only the projection holds the session, and `runtime.sqlite` knows
    /// nothing of it: no committed snapshot and no catalog entry. That is the
    /// one case in which 0.9.3 re-mints a runtime from the projection, so
    /// `--apply` imports the projection copy. Admitted only when that copy
    /// loads, carries the session's id, and its rewrite history validates;
    /// otherwise the row is [`Self::Unreadable`].
    RuntimeMissing,
    /// `runtime.sqlite` has a catalog entry for the session and no
    /// committed snapshot. 0.9.3 does not re-mint such a session; the census
    /// counts it and refuses it.
    RuntimeLifecycleOnly,
    /// The projection extends the parent of a committed rewrite with rows the
    /// runtime never committed, on a session without a lifecycle terminal
    /// fact that 0.9.3 can still write: one a continuity record binds, or any
    /// session of a directory whose projection is `sessions.sqlite3`, a store
    /// that writes without an owner. 0.9.3 rebases those rows over the
    /// rewrite and keeps them, so keeping only Core's copy would lose them.
    ProjectionSuffix,
    /// The same shape on a session with a lifecycle terminal fact: 0.9.3
    /// drops those rows itself, so Core's copy is everything it serves.
    ProjectionSuffixTerminal,
    /// The projection holds messages past the committed head that no rewrite
    /// explains.
    ProjectionAhead,
    /// No proof relates the two copies.
    Divergent,
    /// A copy could not be read, or the projection copy of a session only it
    /// holds does not validate.
    Unreadable,
    /// Core refuses to lay the session's transcript history out as
    /// head-canonical rows (for example a live tail that does not extend the
    /// committed head), so `--apply` could neither convert nor import it. A
    /// graph re-anchored past its oldest rewrites lays out from its retired
    /// base and is admitted. Refused while Core refuses; the census asks
    /// Core's own layout of the retained history.
    LayoutRefused,
    /// The [`Self::ProjectionSuffix`] shape on a session no continuity record
    /// binds, in a continuity layout: 0.9.3 writes such a session only for an
    /// owner a binding record registers, so it can never complete the repair
    /// that keeps those rows, and Core's copy is everything it serves.
    /// "Unbound" reads `continuity_records` only; a mob member binding that
    /// still names the session is the identity census's to refuse.
    ProjectionSuffixUnbound,
    /// A [`Self::RuntimeMissing`] session whose import would find a
    /// different row already in the session store `runtime.sqlite` carries:
    /// the import's create-only write refuses such a row, so the census
    /// refuses it first.
    ImportRowConflict,
    /// `runtime.sqlite`'s WholeBlob authority for the session references a
    /// body the file does not hold, so no release can load the session (and
    /// Core's conversion refuses the file). Refused unless the operator
    /// names the session with `--discard-bodyless`.
    BodilessRuntime,
    /// A [`Self::BodilessRuntime`] the operator named with
    /// `--discard-bodyless`: `--apply` discards that authority.
    BodilessRuntimeDiscarded,
}

impl SessionCensusClass {
    /// Whether migrating keeps everything 0.9.3 would serve for this session.
    pub fn admitted(self) -> bool {
        matches!(
            self,
            Self::Equal
                | Self::ProjectionBehind
                | Self::ProjectionMissing
                | Self::RuntimeMissing
                | Self::ProjectionSuffixTerminal
                | Self::ProjectionSuffixUnbound
                | Self::BodilessRuntimeDiscarded
        )
    }

    /// What `--apply` does with an admitted row beyond keeping Core's copy.
    fn apply_effect(self) -> Option<&'static str> {
        match self {
            Self::RuntimeMissing => Some(
                "--apply imports the projection copy into Core's session store, as 0.9.3 \
                 re-mints the session from it on its next activation",
            ),
            Self::ProjectionSuffixTerminal => Some(
                "--apply keeps Core's copy; the projection's rows past the rewrite's parent are \
                 dropped, as 0.9.3 drops them for a session with a lifecycle terminal fact (the \
                 pre-apply backup of the projection database keeps them)",
            ),
            Self::ProjectionSuffixUnbound => Some(
                "--apply keeps Core's copy; the projection's rows past the rewrite's parent are \
                 dropped: no continuity record binds the session, so 0.9.3 can never write it \
                 and never serves them (the pre-apply backup of continuity.sqlite3 keeps them)",
            ),
            Self::BodilessRuntimeDiscarded => Some(
                "--apply discards the session's WholeBlob authority, which references a missing \
                 body: no release can load the session. The pre-apply backup of runtime.sqlite \
                 keeps the row, and an identity bound to the session starts fresh at its next \
                 wake",
            ),
            _ => None,
        }
    }

    /// What the operator does before rerunning, for a refused row of this
    /// class; [`Self::row_action`] picks the remedy for one row.
    fn action(self) -> Option<&'static str> {
        match self {
            Self::Equal
            | Self::ProjectionBehind
            | Self::ProjectionMissing
            | Self::RuntimeMissing
            | Self::ProjectionSuffixTerminal
            | Self::ProjectionSuffixUnbound
            | Self::BodilessRuntimeDiscarded => None,
            Self::BodilessRuntime => Some(
                "runtime.sqlite's WholeBlob authority for the session references a missing body, \
                 so no release can load it: pass --discard-bodyless <session id> to --apply to \
                 discard that authority (the pre-apply backup keeps it), then rerun",
            ),
            Self::RuntimeLifecycleOnly => Some(
                "runtime.sqlite lists the session but holds no committed copy of it; this \
                 migration does not move that shape: keep the directory on 0.9.3 and report this \
                 census row",
            ),
            // The remedy for a session no 0.9.3 start activates (the shape
            // comes from a retire torn by its own compaction, and the wake of
            // a retired session can be refused), so it names no 0.9.3
            // operation; `row_action` names the verified one for a session a
            // roster identity still runs.
            Self::ProjectionSuffix => Some(
                "the projection holds rows past the rewrite's parent that runtime.sqlite never \
                 committed and 0.9.3 keeps; this migration would drop them: keep the directory \
                 on 0.9.3 and report this census row",
            ),
            Self::ProjectionAhead => Some(
                "the projection holds messages runtime.sqlite never committed; restore \
                 runtime.sqlite from the backup that holds them, or delete the session (or its \
                 identity) with the 0.9.3 binary, then rerun",
            ),
            Self::Divergent => Some(
                "the two copies share no proven history; restore the session from a backup or \
                 delete the identity with the 0.9.3 binary, then rerun",
            ),
            Self::Unreadable => Some("repair or restore the unreadable database, then rerun"),
            Self::LayoutRefused => Some(
                "this migration cannot convert the session's transcript history yet (the detail \
                 names why): keep the directory on 0.9.3 and report this census row",
            ),
            Self::ImportRowConflict => Some(
                "runtime.sqlite already holds a different session store row for the session, so \
                 the import would refuse it: keep the directory on 0.9.3 and report this census \
                 row",
            ),
        }
    }

    /// What the operator does before rerunning, for one refused row. A
    /// suffix on a session one continuity record binds, and that 0.9.3 left
    /// live, converges on a plain 0.9.3 start and clean stop: the identity's
    /// activation runs the repair that re-seeds the committed session
    /// (verified with the published binary for an idle session; the other
    /// live states are expected to converge the same way). That repair is
    /// the continuity layouts' (a builder layout's projection is its
    /// sessions database), so only they name it. Every other row keeps its
    /// class's remedy: 0.9.3 does not activate a retired session at
    /// start.
    fn row_action(
        self,
        identities: &[String],
        runtime_state: Option<RuntimeState>,
        continuity_layout: bool,
    ) -> Option<String> {
        let live = matches!(
            runtime_state,
            Some(
                RuntimeState::Initializing
                    | RuntimeState::Idle
                    | RuntimeState::Attached
                    | RuntimeState::Running
            )
        );
        match (self, identities) {
            (Self::ProjectionSuffix, [identity]) if live && continuity_layout => Some(format!(
                "the projection holds rows past the rewrite's parent that runtime.sqlite never \
                 committed and 0.9.3 keeps; this migration would drop them: keep {identity} on \
                 the roster, start the directory once with 0.9.3 and stop it cleanly; 0.9.3 \
                 re-seeds the session (verified with the published binary for an idle \
                 session); then rerun the census"
            )),
            _ => self.action().map(str::to_owned),
        }
    }
}

/// One classified session.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct SessionCensusRow {
    pub session_id: String,
    pub class: SessionCensusClass,
    pub admitted: bool,
    /// Identities whose continuity record binds this session (roster
    /// layouts only).
    pub identities: Vec<String>,
    pub detail: String,
    /// What `--apply` does with an admitted row beyond keeping Core's copy.
    pub apply_effect: Option<String>,
    /// What the operator does before rerunning, for a refused row.
    pub action: Option<String>,
}

impl SessionCensusRow {
    /// The row of a classified session. `continuity_layout` says whether the
    /// projection is a continuity store a 0.9.3 start of this directory
    /// serves (the restart remedy applies only there).
    pub(super) fn new(
        session_id: String,
        class: SessionCensusClass,
        identities: Vec<String>,
        detail: String,
        runtime_state: Option<RuntimeState>,
        continuity_layout: bool,
    ) -> Self {
        Self {
            admitted: class.admitted(),
            apply_effect: class.apply_effect().map(str::to_owned),
            action: class.row_action(&identities, runtime_state, continuity_layout),
            identities,
            session_id,
            class,
            detail,
        }
    }
}

/// Where the identity census stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityCensusStatus {
    /// Not reached: the run refused before the session census finished.
    NotRun,
    /// The session census ran; comparing each identity with Core's member
    /// history needs Core's read-only member-history reader, which this
    /// build does not have yet.
    PendingCoreMemberHistory,
    /// The layout keeps no identity binding (no roster provider), so there
    /// is no identity to compare.
    NoIdentities,
    /// Every identity was compared; [`NativeMigrateReport::identities`]
    /// holds the rows.
    Ran,
}

/// A refusal of the whole run. Nothing is written when any is present,
/// except [`NativeMigrateRefusal::ApplyFailed`], which an apply run reports
/// after its journal is written, and the dropped journal of a run that
/// stopped before `runtime.sqlite` was converted.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NativeMigrateRefusal {
    /// The state directory does not exist or is not a directory.
    NotADirectory,
    /// Two spellings of one database are present (the canonical name and a
    /// legacy one, or two legacy ones). The 0.9.4 layout refuses them too;
    /// `meta_gateway storage-migrate --apply` resolves the twins. A lone
    /// legacy spelling is read in place, as the layout resolves it.
    SpellingTwins { slot: String, files: Vec<String> },
    /// No `runtime.sqlite`: there is no committed session authority to keep.
    NoRuntimeStore,
    /// Neither projection database is present. If 0.9.3 ever ran with
    /// `store_path` naming a file, run `meta_gateway storage-migrate
    /// --state-dir <dir> --session-db <file> --apply` first.
    NoProjection,
    /// Both `sessions.sqlite3` and `continuity.sqlite3` hold rows a release
    /// reads: a session body in the continuity file, and in the sessions file
    /// a session or another row its retirement would lose. Nothing says which
    /// copy 0.9.3 served.
    AmbiguousProjection,
    /// The maintenance fence could not be taken: an operation another
    /// process had in flight did not drain. The fence detects no idle process:
    /// [`Self::RuntimeInUse`] detects one that hosts a runtime, and while the
    /// fence is held every store operation of any other process fails.
    FenceUnavailable { detail: String },
    /// A process holds a runtime lock of the directory: a 0.9.3 process that
    /// hosts at least one runtime (Core's `runtime.sqlite.execution-custody`,
    /// claimed per runtime driver) or a 0.9.4 Meta runtime
    /// (`meta-runtime.lock`). Stop every process that uses the directory.
    RuntimeInUse { file: String },
    /// A runtime lock file of the directory could not be probed.
    RuntimeLockUnreadable { file: String, detail: String },
    /// A `--apply` run has not finished; its journal names the steps done.
    MigrationIncomplete { journal: PathBuf },
    /// A scratch copy could not be made.
    ScratchCopyFailed { file: String, detail: String },
    /// The runtime store copy could not be opened or listed.
    RuntimeStoreUnreadable { detail: String },
    /// `runtime.sqlite` holds a WholeBlob runtime that is not a session's.
    /// A 0.9.3 gateway writes session runtimes only.
    ForeignRuntime { runtime_id: String },
    /// The projection copy could not be opened or listed.
    ProjectionUnreadable { detail: String },
    /// At least one session row is not admitted.
    SessionsRefused { count: usize },
    /// The identity census has not run (see [`IdentityCensusStatus`]). Not
    /// reported any more: the identity census runs for every roster layout.
    /// The variant stays so the refusal kinds keep their names.
    IdentityCensusPending,
    /// `--apply` cannot carry out this directory. Not reported any more:
    /// `--apply` imports the sessions only the projection holds. The variant
    /// stays so the refusal kinds keep their names.
    ApplyNotAvailable,
    /// `--mob-store` names a file that does not exist.
    MobStoreMissing { path: PathBuf },
    /// `--mob-store` names a store other than the directory's own
    /// `event_log.sqlite3`, which exists: the directory's mob lives in one of
    /// them, and the census cannot tell which.
    MobStoreConflict { named: PathBuf, found: PathBuf },
    /// The directory holds the composition manifest of a mob store other
    /// than `event_log.sqlite3` (0.9.3 writes one beside every durable mob
    /// store and nowhere else), and no `--mob-store` names the store. Pass
    /// `--mob-store <path>`.
    MobStoreUnnamed { manifests: Vec<String> },
    /// The state directory could not be listed to look for mob stores, or
    /// the mob store's member binding history could not be read.
    MobStorageUnreadable { detail: String },
    /// `--apply` stopped part way; `detail` names where. Its journal stays in
    /// the directory and a 0.9.4 runtime refuses to start there. Run
    /// `--apply` again: it finishes the run once Core has pinned
    /// `runtime.sqlite` for the conversion, and starts over with a new census
    /// before that. Or restore the backups the journal lists and remove the
    /// journal.
    ApplyFailed { detail: String },
    /// The journal an earlier `--apply` left could not be read while
    /// `runtime.sqlite` carries Core's claim, so the run cannot tell how far
    /// the conversion got. Nothing was written. A 0.9.4 build that reads the
    /// journal can finish it. Otherwise restore `runtime.sqlite` and each
    /// projection file from their `*.pre-*.native-migration` backups, then
    /// remove the journal: removing the journal alone could leave a partly
    /// converted `runtime.sqlite` that neither release reads. Without the
    /// claim, `--apply` drops such a journal and starts over.
    MigrationJournalUnreadable { detail: String },
    /// The directory holds the composition manifest 0.9.3 writes beside its
    /// own `event_log.sqlite3`, but not the store, and no `--mob-store`
    /// names one. 0.9.3 writes that manifest only beside a durable store, so
    /// the store was moved or deleted, and reading the mob as in memory
    /// would lose its bindings. Restore the store, or name it with
    /// `--mob-store <path>`.
    MobStoreManifestWithoutStore { manifest: String, store: PathBuf },
    /// `--apply` could not write its journal, its first write. Nothing was
    /// written; run `--apply` again once `detail` is resolved.
    ApplyNotStarted { detail: String },
    /// A file under a name no Meta release gives a database holds the
    /// session store's schema (a 0.9.3 `store_path` override, or a copy).
    /// The text report names the remedy: `meta_gateway storage-migrate
    /// --session-db` first, or move the file out of the state directory.
    UnrecognizedSessionDatabase { file: String },
    /// A file the state-directory scan had to read could not be read; the
    /// scan fails closed.
    StateFileUnreadable { file: String, detail: String },
    /// `--discard-bodyless` named a session `runtime.sqlite` holds whose
    /// WholeBlob authority the census did not find body-less; nothing is
    /// discarded on a guess. A named session `runtime.sqlite` no longer holds
    /// at all (an earlier run discarded it and stopped before Core's pin) is
    /// not refused.
    DiscardNotBodiless { session_id: String },
    /// The continuity export file could not be read, or is not a valid
    /// export (the detail names the check it failed).
    ContinuityExportUnreadable { detail: String },
    /// The state directory cannot take a continuity export: it holds a local
    /// projection file, store files without `runtime.sqlite`, or a store
    /// 0.9.4 already uses.
    ContinuityExportDirectory { detail: String },
    /// At least one identity row is not admitted.
    IdentitiesRefused { count: usize },
    /// `--apply` over an unfinished export run named another file than the
    /// one that run started with (its entries digest differs), or the
    /// unfinished run imported no export. Nothing was written.
    ContinuityExportChanged { detail: String },
}

/// The outcome of one `storage-migrate-native` run.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct NativeMigrateReport {
    pub mode: MigrateMode,
    pub state_dir: PathBuf,
    /// The startup predicate's verdict on the directory, read first. Only a
    /// directory that needs migration is censused.
    pub layout: Option<NativeLayoutStatus>,
    pub shape: Option<GatewayStateShape>,
    /// The mob storage the identity rules read, once the census found it.
    pub mob_storage: Option<CensusMobStore>,
    pub sessions: Vec<SessionCensusRow>,
    pub identity_census: IdentityCensusStatus,
    /// One row per identity, once the identity census ran.
    pub identities: Vec<IdentityCensusRow>,
    /// The first-boot manifest rows `--apply` writes for the admitted
    /// identities.
    pub first_boot: Vec<FirstBootRowPlan>,
    /// The continuity export the run read, with `--continuity-export`.
    pub continuity_export: Option<ContinuityExportSummary>,
    pub refusals: Vec<NativeMigrateRefusal>,
    /// What `--apply` carried out, when it ran to the end.
    pub applied: Option<NativeApplyOutcome>,
}

impl NativeMigrateReport {
    pub(super) fn new(mode: MigrateMode, state_dir: &Path) -> Self {
        Self {
            mode,
            state_dir: state_dir.to_path_buf(),
            layout: None,
            shape: None,
            mob_storage: None,
            sessions: Vec::new(),
            identity_census: IdentityCensusStatus::NotRun,
            applied: None,
            identities: Vec::new(),
            first_boot: Vec::new(),
            continuity_export: None,
            refusals: Vec::new(),
        }
    }

    /// True when the run refused; the verb then exits non-zero.
    pub fn has_refusals(&self) -> bool {
        !self.refusals.is_empty()
    }

    /// The number of rows of each class, for the text report.
    pub fn class_counts(&self) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for row in &self.sessions {
            let name = serde_json::to_value(row.class)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| format!("{:?}", row.class));
            *counts.entry(name).or_insert(0) += 1;
        }
        counts
    }
}

/// One `storage-migrate-native` run.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NativeMigrateRequest {
    pub state_dir: PathBuf,
    pub mode: MigrateMode,
    /// Where the scratch copies go. `None` puts them beside the state
    /// directory, in its parent, so they stay on its filesystem and in its
    /// trust domain.
    pub scratch_dir: Option<PathBuf>,
    /// The durable mob store of an application that kept it somewhere other
    /// than the directory's `event_log.sqlite3`. `None` looks for the store
    /// in the directory.
    pub mob_store: Option<PathBuf>,
    /// A continuity export file the standalone export scripts wrote: the
    /// projection side of a host whose sessions a continuity provider held.
    pub continuity_export: Option<PathBuf>,
    /// Sessions whose body-less WholeBlob authority `--apply` may discard
    /// (`--discard-bodyless`). Naming a session the census does not find
    /// body-less refuses the run.
    pub discard_bodyless: BTreeSet<String>,
}

impl NativeMigrateRequest {
    pub fn new(state_dir: impl Into<PathBuf>, mode: MigrateMode) -> Self {
        Self {
            state_dir: state_dir.into(),
            mode,
            scratch_dir: None,
            mob_store: None,
            continuity_export: None,
            discard_bodyless: BTreeSet::new(),
        }
    }

    /// Let `--apply` discard the body-less WholeBlob authority of
    /// `session_id`.
    #[must_use]
    pub fn with_discard_bodyless(mut self, session_id: impl Into<String>) -> Self {
        self.discard_bodyless.insert(session_id.into());
        self
    }

    /// Put the scratch copies under `dir` instead of beside the state
    /// directory.
    #[must_use]
    pub fn with_scratch_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.scratch_dir = Some(dir.into());
        self
    }

    /// Name the durable mob store the directory's mob lives in. A relative
    /// path is made absolute against the working directory here, without
    /// resolving symlinks, so the report and anything `--apply` records name
    /// the same file whatever directory reads them later.
    #[must_use]
    pub fn with_mob_store(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        self.mob_store = Some(std::path::absolute(&path).unwrap_or(path));
        self
    }

    /// Migrate from the continuity export file at `path` instead of a local
    /// projection.
    #[must_use]
    pub fn with_continuity_export(mut self, path: impl Into<PathBuf>) -> Self {
        self.continuity_export = Some(path.into());
        self
    }
}

/// Run the native-storage migration over one state directory.
///
/// A directory the startup predicate reads as native has nothing to migrate
/// and reports no refusal. A dry run reports the census. [`MigrateMode::Apply`]
/// carries out a clean census. A run an earlier `--apply` journaled is
/// finished from its journal once `runtime.sqlite` carries Core's durable
/// claim, which Core writes before its first conversion write and 0.9.3
/// refuses; before that the journal is dropped and the run starts over with
/// a new census (see `apply`).
pub async fn migrate_state_dir_native(request: &NativeMigrateRequest) -> NativeMigrateReport {
    if let Some(export) = request.continuity_export.as_deref() {
        return super::export_census::census_continuity_export(request, export).await;
    }
    let state_dir = request.state_dir.as_path();
    let mode = request.mode;
    let mut report = NativeMigrateReport::new(mode, state_dir);
    if !state_dir.is_dir() {
        report.refusals.push(NativeMigrateRefusal::NotADirectory);
        return report;
    }
    let mut layout = native_layout_status(state_dir);
    let mut restarted = false;
    if let (NativeLayoutStatus::MigrationIncomplete { .. }, MigrateMode::Apply) = (&layout, mode) {
        // Core pins runtime.sqlite before its first conversion write, so a
        // partly converted file always carries the claim, and 0.9.3 refuses
        // it: nothing can have written the directory since. Without the
        // claim 0.9.3 still opens the directory, whatever the journal says
        // and whether or not it can be read.
        match runtime_conversion_began(state_dir) {
            // Finish the run from its journal, with the backups it took. A
            // journal that cannot be read is refused there: the run cannot
            // tell how far the conversion got.
            Ok(true) => {
                report.layout = Some(layout);
                finish_apply(state_dir, &mut report).await;
                return report;
            }
            // At most the session-store schema (which 0.9.3 ignores) and a
            // discarded body-less runtime were written to runtime.sqlite,
            // and 0.9.3 may have written the directory since: drop the
            // journal unread (its backups stay) and start over with a new
            // census and new backups.
            Ok(false) => {
                if let Err(detail) = apply::abandon_journal(state_dir) {
                    report.layout = Some(layout);
                    report
                        .refusals
                        .push(NativeMigrateRefusal::ApplyFailed { detail });
                    return report;
                }
                restarted = true;
                layout = native_layout_status(state_dir);
            }
            Err(detail) => {
                report.layout = Some(layout);
                report
                    .refusals
                    .push(NativeMigrateRefusal::RuntimeStoreUnreadable { detail });
                return report;
            }
        }
    }
    report.layout = Some(layout.clone());
    match layout {
        NativeLayoutStatus::Native => return report,
        NativeLayoutStatus::MigrationIncomplete { journal } => {
            report
                .refusals
                .push(NativeMigrateRefusal::MigrationIncomplete { journal });
            return report;
        }
        NativeLayoutStatus::NeedsMigration { .. } => {}
    }
    match unrecognized_session_databases(state_dir) {
        Ok(found) if found.is_empty() => {}
        Ok(found) => {
            report
                .refusals
                .extend(found.into_iter().map(|found| match found {
                    UnrecognizedFile::SessionDatabase { file } => {
                        NativeMigrateRefusal::UnrecognizedSessionDatabase { file }
                    }
                    UnrecognizedFile::Unreadable { file, detail } => {
                        NativeMigrateRefusal::StateFileUnreadable { file, detail }
                    }
                }));
            return report;
        }
        Err(detail) => {
            report
                .refusals
                .push(NativeMigrateRefusal::StateFileUnreadable {
                    file: state_dir.display().to_string(),
                    detail,
                });
            return report;
        }
    }
    let (projections, files) = match detect_projections(state_dir) {
        Ok(detected) => detected,
        Err(refusals) => {
            report.refusals.extend(refusals);
            return report;
        }
    };
    let mob_storage = match resolve_mob_storage(state_dir, request.mob_store.as_deref()) {
        Ok(mob_storage) => mob_storage,
        Err(refusal) => {
            report.refusals.push(refusal);
            return report;
        }
    };
    let mut shape = projections.shape(&mob_storage);
    report.shape = Some(shape);
    report.mob_storage = Some(mob_storage);

    let scratch_parent = match (&request.scratch_dir, state_dir.parent()) {
        (Some(dir), _) => dir.clone(),
        (None, Some(parent)) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        (None, _) => PathBuf::from("."),
    };
    let scratch = match tempfile::Builder::new()
        .prefix(".meta-storage-migrate-native-")
        .tempdir_in(&scratch_parent)
    {
        Ok(scratch) => scratch,
        Err(error) => {
            report
                .refusals
                .push(NativeMigrateRefusal::ScratchCopyFailed {
                    file: String::new(),
                    detail: format!("scratch directory in {}: {error}", scratch_parent.display()),
                });
            return report;
        }
    };
    // Held while copying, and by an apply run to its last step. The runtime
    // locks keep a runtime from starting meanwhile; the fence drains
    // operations already in flight and blocks new ones, so the copies are
    // one consistent snapshot of the files the apply run then changes.
    let held = match hold_directory(state_dir).await {
        Ok(held) => held,
        Err(refusal) => {
            report.refusals.push(refusal);
            return report;
        }
    };
    for (file, scratch_name) in copied_files(shape, files) {
        if let Err(detail) = copy_database(state_dir, scratch.path(), file, scratch_name) {
            report
                .refusals
                .push(NativeMigrateRefusal::ScratchCopyFailed {
                    file: file.to_owned(),
                    detail,
                });
            return report;
        }
    }
    // The identity census reads Core's member binding history from a copy
    // of the durable mob store, taken under the same fence.
    let mob_copy = match report.mob_storage.as_ref() {
        Some(CensusMobStore::Durable { path })
            if shape != GatewayStateShape::SessionsProjection =>
        {
            let copy = scratch.path().join(MOB_STORE_SCRATCH_NAME);
            if let Err(detail) = super::export_census::copy_with_wal(path, &copy) {
                report
                    .refusals
                    .push(NativeMigrateRefusal::ScratchCopyFailed {
                        file: path.display().to_string(),
                        detail,
                    });
                return report;
            }
            Some(copy)
        }
        _ => None,
    };
    let held = matches!(mode, MigrateMode::Apply).then_some(held);

    if shape == GatewayStateShape::SessionsProjectionWithBindings {
        // Decided on the copies. A session body in continuity.sqlite3 makes
        // it the projection, unless sessions.sqlite3 holds a row a release
        // reads too: then both files hold sessions, and nothing says which
        // copy 0.9.3 served.
        match continuity_session_ids(&scratch.path().join(CANONICAL_CONTINUITY_DB_FILE_NAME)) {
            Ok(bodies) if bodies.is_empty() => {}
            Ok(_) => match sessions_database_holds_rows(
                &scratch.path().join(CANONICAL_SESSIONS_DB_FILE_NAME),
            ) {
                // No session and nothing else a release reads: the sessions
                // file is no projection. `--apply` retires it with its stamp,
                // and its pre-apply backup keeps the rows it held.
                Ok(false) => {
                    if let Some(mob_storage) = report.mob_storage.as_ref() {
                        shape = Projections::Continuity.shape(mob_storage);
                        report.shape = Some(shape);
                    }
                }
                Ok(true) => {
                    report.shape = None;
                    report
                        .refusals
                        .push(NativeMigrateRefusal::AmbiguousProjection);
                    return report;
                }
                Err(detail) => {
                    report.shape = None;
                    report
                        .refusals
                        .push(NativeMigrateRefusal::ProjectionUnreadable { detail });
                    return report;
                }
            },
            Err(detail) => {
                report.shape = None;
                report
                    .refusals
                    .push(NativeMigrateRefusal::ProjectionUnreadable { detail });
                return report;
            }
        }
    }

    let runtime = match open_runtime_sessions(&scratch.path().join(RUNTIME_DB_FILE_NAME)).await {
        Ok(runtime) => runtime,
        Err(refusal) => {
            report.refusals.push(refusal);
            return report;
        }
    };
    let projection = match open_projection(shape, scratch.path()).await {
        Ok(projection) => projection,
        Err(detail) => {
            report
                .refusals
                .push(NativeMigrateRefusal::ProjectionUnreadable { detail });
            return report;
        }
    };

    let session_ids: BTreeSet<String> = runtime
        .known_session_ids()
        .chain(projection.session_ids.iter().cloned())
        .chain(projection.bindings.keys().cloned())
        .collect();
    let known_sessions: BTreeSet<SessionId> = session_ids
        .iter()
        .filter_map(|session_id| SessionId::parse(session_id).ok())
        .collect();
    for session_id in session_ids {
        // A record that binds a session without a body is no copy.
        let in_projection = projection.session_ids.contains(&session_id);
        let identities = projection
            .bindings
            .get(&session_id)
            .cloned()
            .unwrap_or_default();
        let (mut class, mut detail) = classify_session(
            &session_id,
            &runtime,
            in_projection,
            &projection.store,
            shape,
            !identities.is_empty(),
        )
        .await;
        if class == SessionCensusClass::BodilessRuntime
            && request.discard_bodyless.contains(&session_id)
        {
            (class, detail) =
                discarded_bodiless(&session_id, in_projection, &projection.store, detail).await;
        }
        report.sessions.push(SessionCensusRow {
            admitted: class.admitted(),
            apply_effect: class.apply_effect().map(str::to_owned),
            action: class.row_action(
                &identities,
                runtime.runtime_state(&session_id),
                matches!(
                    shape,
                    GatewayStateShape::ContinuityProjection
                        | GatewayStateShape::ContinuityProjectionInMemoryMob
                ),
            ),
            identities,
            session_id,
            class,
            detail,
        });
    }
    if let Err(detail) = predict_import_rows(
        Some(&scratch.path().join(RUNTIME_DB_FILE_NAME)),
        scratch.path(),
        &projection.store,
        &mut report.sessions,
    )
    .await
    {
        report
            .refusals
            .push(NativeMigrateRefusal::RuntimeStoreUnreadable { detail });
        return report;
    }
    // A named session runtime.sqlite still holds, and not as a body-less
    // runtime, refuses. One it no longer holds at all was discarded by an
    // earlier run that stopped before the pin: naming it again is a no-op.
    for session_id in &request.discard_bodyless {
        let held = runtime.committed.contains_key(session_id)
            || runtime.catalog.contains_key(session_id)
            || runtime.unreadable.contains_key(session_id);
        if held && !runtime.bodiless.contains_key(session_id) {
            report
                .refusals
                .push(NativeMigrateRefusal::DiscardNotBodiless {
                    session_id: session_id.clone(),
                });
        }
    }
    let refused = report.sessions.iter().filter(|row| !row.admitted).count();
    if refused != 0 {
        report
            .refusals
            .push(NativeMigrateRefusal::SessionsRefused { count: refused });
    }
    if shape == GatewayStateShape::SessionsProjection {
        report.identity_census = IdentityCensusStatus::NoIdentities;
    } else {
        // Every continuity record is a claim; the directory's records list
        // every identity 0.9.3 knew.
        let history = match &mob_copy {
            Some(copy) => match meerkat_mob::MobStorage::read_member_binding_history(copy).await {
                Ok(history) => Some(history),
                Err(error) => {
                    report
                        .refusals
                        .push(NativeMigrateRefusal::MobStorageUnreadable {
                            detail: error.to_string(),
                        });
                    return report;
                }
            },
            None => None,
        };
        let claims: Vec<IdentityClaim> = projection
            .bindings
            .iter()
            .flat_map(|(session_id, identities)| {
                let session = SessionId::parse(session_id).ok();
                identities.iter().map(move |identity| IdentityClaim {
                    identity: identity.clone(),
                    session: session.clone(),
                    values_093: None,
                })
            })
            .collect();
        let census = identity_census(
            &claims,
            ClaimSource::ContinuityRecords,
            history.as_ref(),
            &known_sessions,
        );
        let refused = census.refused();
        report.identities = census.rows;
        report.first_boot = census.first_boot;
        report.identity_census = IdentityCensusStatus::Ran;
        if refused != 0 {
            report
                .refusals
                .push(NativeMigrateRefusal::IdentitiesRefused { count: refused });
        }
    }
    if let Some(held) = held {
        if report.refusals.is_empty() {
            let meta_lock_held = held.holds_meta_runtime_lock(state_dir);
            let discard = runtime
                .bodiless
                .iter()
                .filter(|(session_id, _)| request.discard_bodyless.contains(*session_id))
                .flat_map(|(session_id, runtime_ids)| {
                    runtime_ids.iter().map(|runtime_id| DiscardedRuntime {
                        session_id: session_id.clone(),
                        runtime_id: runtime_id.clone(),
                    })
                })
                .collect();
            apply_census(
                state_dir,
                shape,
                files,
                discard,
                meta_lock_held,
                &mut report,
            )
            .await;
            if let Some(applied) = report.applied.as_mut() {
                applied.restarted = restarted;
            }
        }
        drop(held);
    }
    report
}

/// The directory's runtime locks and maintenance fence, held together.
pub(super) struct HeldDirectory {
    runtimes: LiveRuntimeExclusion,
    _fence: MetaMaintenanceFence,
}

impl HeldDirectory {
    /// Whether the hold already locks the directory's Meta runtime lock
    /// file (it existed when the hold was taken, as after a stopped run).
    pub(super) fn holds_meta_runtime_lock(&self, state_dir: &Path) -> bool {
        self.runtimes
            .holds(&state_dir.join(META_RUNTIME_LOCK_FILE_NAME))
    }
}

pub(super) async fn hold_directory(
    state_dir: &Path,
) -> Result<HeldDirectory, NativeMigrateRefusal> {
    let runtimes = exclude_live_runtimes(state_dir)?;
    let state_dir_owned = state_dir.to_path_buf();
    let fence = tokio::task::spawn_blocking(move || {
        MetaMaintenanceFence::acquire(&state_dir_owned, FENCE_DRAIN_DEADLINE)
    })
    .await;
    match fence {
        Ok(Ok(fence)) => Ok(HeldDirectory {
            runtimes,
            _fence: fence,
        }),
        Ok(Err(error)) => Err(NativeMigrateRefusal::FenceUnavailable {
            detail: error.to_string(),
        }),
        Err(error) => Err(NativeMigrateRefusal::FenceUnavailable {
            detail: format!("fence task: {error}"),
        }),
    }
}

/// Carry out a clean census, while the caller holds the directory. This
/// build applies a directory whose every session `runtime.sqlite` already
/// holds; the census refuses identity layouts before this point.
async fn apply_census(
    state_dir: &Path,
    shape: GatewayStateShape,
    files: ProjectionFiles,
    discard: Vec<DiscardedRuntime>,
    meta_lock_held: bool,
    report: &mut NativeMigrateReport,
) {
    // The sessions only the projection holds come from its pre-apply
    // backup, the copy the run keeps until it is done.
    let imports = runtime_missing_sessions(report);
    // The projection's own file name, a legacy spelling read in place
    // included: the journal keys its backup by that name.
    let import_source = (!imports.is_empty()).then(|| match shape {
        GatewayStateShape::SessionsProjection
        | GatewayStateShape::SessionsProjectionWithBindings => ImportSource::Projection {
            file: files
                .sessions
                .unwrap_or(CANONICAL_SESSIONS_DB_FILE_NAME)
                .to_owned(),
            store: ProjectionStore::Sessions,
        },
        _ => ImportSource::Projection {
            file: files
                .continuity
                .unwrap_or(CANONICAL_CONTINUITY_DB_FILE_NAME)
                .to_owned(),
            store: ProjectionStore::Continuity,
        },
    });
    let plan = ApplyPlan {
        projections: retired_projections(shape, files),
        imports,
        import_source,
        first_boot: report.first_boot.clone(),
        new_directory: false,
        discard,
    };
    match ApplyJournal::start(state_dir, plan) {
        Ok(journal) => run_journal(state_dir, journal, false, meta_lock_held, report).await,
        Err(detail) => report
            .refusals
            .push(NativeMigrateRefusal::ApplyNotStarted { detail }),
    }
}

/// The sessions a clean census imports: its `RuntimeMissing` rows, in
/// session order.
pub(super) fn runtime_missing_sessions(report: &NativeMigrateReport) -> Vec<String> {
    report
        .sessions
        .iter()
        .filter(|row| row.class == SessionCensusClass::RuntimeMissing)
        .map(|row| row.session_id.clone())
        .collect()
}

/// Finish the run an earlier `--apply` journaled.
pub(super) async fn finish_apply(state_dir: &Path, report: &mut NativeMigrateReport) {
    let held = match hold_directory(state_dir).await {
        Ok(held) => held,
        Err(refusal) => {
            report.refusals.push(refusal);
            return;
        }
    };
    match ApplyJournal::read(state_dir) {
        Ok(journal) => {
            let meta_lock_held = held.holds_meta_runtime_lock(state_dir);
            run_journal(state_dir, journal, true, meta_lock_held, report).await;
        }
        Err(detail) => report
            .refusals
            .push(NativeMigrateRefusal::MigrationJournalUnreadable { detail }),
    }
    drop(held);
}

pub(super) async fn run_journal(
    state_dir: &Path,
    mut journal: ApplyJournal,
    resumed: bool,
    meta_lock_held: bool,
    report: &mut NativeMigrateReport,
) {
    let dir = state_dir.to_path_buf();
    let ran = tokio::task::spawn_blocking(move || {
        let mut outcome = NativeApplyOutcome {
            resumed,
            ..NativeApplyOutcome::default()
        };
        apply::run(&dir, &mut journal, &mut outcome, meta_lock_held).map(|()| outcome)
    })
    .await;
    match ran {
        Ok(Ok(outcome)) => report.applied = Some(outcome),
        Ok(Err(detail)) => report
            .refusals
            .push(NativeMigrateRefusal::ApplyFailed { detail }),
        Err(error) => report.refusals.push(NativeMigrateRefusal::ApplyFailed {
            detail: format!("apply task: {error}"),
        }),
    }
}

/// The projection databases a directory of `shape` retires, and how. The
/// sessions database takes its retired stamp. The continuity database only
/// loses its session tables' rows: a 0.9.4 identity gateway still opens it
/// for its identity records and would refuse a stamp.
fn retired_projections(shape: GatewayStateShape, files: ProjectionFiles) -> Vec<RetiredProjection> {
    let sessions = || RetiredProjection {
        file: files
            .sessions
            .unwrap_or(CANONICAL_SESSIONS_DB_FILE_NAME)
            .to_owned(),
        domain: SESSION_STORE_DOMAIN.name.to_owned(),
        retirement: Retirement::Stamp,
    };
    let continuity = || RetiredProjection {
        file: files
            .continuity
            .unwrap_or(CANONICAL_CONTINUITY_DB_FILE_NAME)
            .to_owned(),
        domain: CONTINUITY_SCHEMA_DOMAIN.to_owned(),
        retirement: Retirement::StripSessionTables,
    };
    match shape {
        GatewayStateShape::SessionsProjection => vec![sessions()],
        // A sessions file beside the continuity projection held no session
        // and nothing else a release reads (decided on the census copies). It
        // takes its retired stamp too: 0.9.4 refuses a sessions database
        // that is not retired.
        GatewayStateShape::ContinuityProjection
        | GatewayStateShape::ContinuityProjectionInMemoryMob => match files.sessions {
            Some(_) => vec![continuity(), sessions()],
            None => vec![continuity()],
        },
        GatewayStateShape::SessionsProjectionWithBindings => vec![sessions(), continuity()],
        // An export's directory holds no local projection.
        GatewayStateShape::ContinuityExportNew | GatewayStateShape::ContinuityExportInPlace => {
            Vec::new()
        }
    }
}

/// The projection databases a 0.9.3 directory holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Projections {
    Sessions,
    Continuity,
    /// Both files: the builder layout when `continuity.sqlite3` holds no
    /// session body, the continuity layout when `sessions.sqlite3` holds no
    /// row a release reads, ambiguous otherwise (decided on the copies).
    Both,
}

impl Projections {
    fn shape(self, mob_storage: &CensusMobStore) -> GatewayStateShape {
        match (self, mob_storage) {
            (Self::Sessions, _) => GatewayStateShape::SessionsProjection,
            (Self::Continuity, CensusMobStore::Durable { .. }) => {
                GatewayStateShape::ContinuityProjection
            }
            (Self::Continuity, CensusMobStore::InMemory) => {
                GatewayStateShape::ContinuityProjectionInMemoryMob
            }
            (Self::Both, _) => GatewayStateShape::SessionsProjectionWithBindings,
        }
    }
}

fn detect_projections(
    state_dir: &Path,
) -> Result<(Projections, ProjectionFiles), Vec<NativeMigrateRefusal>> {
    let mut refusals = Vec::new();
    let mut files = ProjectionFiles::default();
    match projection_file(state_dir, DatabaseSlot::Sessions) {
        Ok(file) => files.sessions = file,
        Err(refusal) => refusals.push(refusal),
    }
    match projection_file(state_dir, DatabaseSlot::Continuity) {
        Ok(file) => files.continuity = file,
        Err(refusal) => refusals.push(refusal),
    }
    if !refusals.is_empty() {
        return Err(refusals);
    }
    if !state_dir.join(RUNTIME_DB_FILE_NAME).is_file() {
        return Err(vec![NativeMigrateRefusal::NoRuntimeStore]);
    }
    let projections = match (files.sessions, files.continuity) {
        (Some(_), Some(_)) => Projections::Both,
        (Some(_), None) => Projections::Sessions,
        (None, Some(_)) => Projections::Continuity,
        (None, None) => return Err(vec![NativeMigrateRefusal::NoProjection]),
    };
    Ok((projections, files))
}

/// The file each projection slot uses in the state directory: the canonical
/// name or a lone legacy spelling, exactly as the 0.9.4 layout resolves it
/// (`MetaStorageLayout::resolve_database`), so the census reads and the
/// apply retires the file the 0.9.4 gateways open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ProjectionFiles {
    sessions: Option<&'static str>,
    continuity: Option<&'static str>,
}

fn projection_file(
    state_dir: &Path,
    slot: DatabaseSlot,
) -> Result<Option<&'static str>, NativeMigrateRefusal> {
    let domain = match slot {
        DatabaseSlot::Continuity => CONTINUITY_SCHEMA_DOMAIN,
        _ => SESSION_STORE_DOMAIN.name,
    };
    // A retired stamp (the native apply's, or storage-migrate --session-db's
    // tripwire at a legacy spelling) is neither a projection nor a twin. A
    // file that cannot be read counts as present: the census then refuses
    // it rather than skip it.
    let present: Vec<&'static str> = std::iter::once(slot.canonical_name())
        .chain(slot.legacy_names().iter().copied())
        .filter(|name| {
            let path = state_dir.join(name);
            path.exists() && !matches!(is_retired_stamp_file(&path, domain), Ok(true))
        })
        .collect();
    match present.as_slice() {
        [] => Ok(None),
        [file] => Ok(Some(*file)),
        _ => Err(NativeMigrateRefusal::SpellingTwins {
            slot: slot.canonical_name().to_owned(),
            files: present.iter().map(|file| (*file).to_owned()).collect(),
        }),
    }
}

/// The suffix of the composition manifest 0.9.3 writes beside a durable mob
/// store (`<store>.composition.json`), and nowhere else.
const COMPOSITION_MANIFEST_SUFFIX: &str = ".composition.json";

/// The directory's mob storage: the store `--mob-store` names, else
/// `event_log.sqlite3` in the directory, else none. A composition manifest
/// of any other store in the directory means a durable store the census was
/// not told about, so the run refuses rather than guess; so does the
/// manifest of `event_log.sqlite3` without the store, a named store that
/// does not exist, or one that is not the directory's own
/// `event_log.sqlite3` when that exists. A store outside the directory is
/// found only through `--mob-store`.
pub(super) fn resolve_mob_storage(
    state_dir: &Path,
    named: Option<&Path>,
) -> Result<CensusMobStore, NativeMigrateRefusal> {
    let own = state_dir.join(EVENT_LOG_DB_FILE_NAME);
    if let Some(named) = named {
        if !named.is_file() {
            return Err(NativeMigrateRefusal::MobStoreMissing {
                path: named.to_path_buf(),
            });
        }
        if own.is_file() && !same_file(named, &own) {
            return Err(NativeMigrateRefusal::MobStoreConflict {
                named: named.to_path_buf(),
                found: own,
            });
        }
        return Ok(CensusMobStore::Durable {
            path: named.to_path_buf(),
        });
    }
    let unreadable = |error: std::io::Error| NativeMigrateRefusal::MobStorageUnreadable {
        detail: format!("{}: {error}", state_dir.display()),
    };
    let own_manifest = format!("{EVENT_LOG_DB_FILE_NAME}{COMPOSITION_MANIFEST_SUFFIX}");
    let mut manifests = Vec::new();
    let mut own_manifest_found = false;
    for entry in std::fs::read_dir(state_dir).map_err(unreadable)? {
        let name = entry.map_err(unreadable)?.file_name();
        let name = name.to_string_lossy();
        if name == own_manifest {
            own_manifest_found = true;
        } else if name.ends_with(COMPOSITION_MANIFEST_SUFFIX) {
            manifests.push(name.into_owned());
        }
    }
    if !manifests.is_empty() {
        manifests.sort();
        return Err(NativeMigrateRefusal::MobStoreUnnamed { manifests });
    }
    if own.is_file() {
        Ok(CensusMobStore::Durable { path: own })
    } else if own_manifest_found {
        Err(NativeMigrateRefusal::MobStoreManifestWithoutStore {
            manifest: own_manifest,
            store: own,
        })
    } else {
        Ok(CensusMobStore::InMemory)
    }
}

/// Whether two existing paths resolve to the same path, symlinks followed.
/// Two hard links to one file resolve to different paths, so naming the
/// directory's own store through a hard link refuses as a conflict, which
/// is the safe side. A path that does not resolve names no file the other
/// does.
fn same_file(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Exclusive locks on the directory's runtime lock files, held while the
/// census copies it.
pub(crate) struct LiveRuntimeExclusion {
    _locks: Vec<std::fs::File>,
    /// The lock files held, by path.
    held: Vec<PathBuf>,
}

impl LiveRuntimeExclusion {
    /// Whether this exclusion holds the lock file at `path`. A second open
    /// of a file this process already locks would refuse itself: the lock
    /// belongs to the open file, not to the process.
    pub(super) fn holds(&self, path: &Path) -> bool {
        self.held.iter().any(|held| held == path)
    }
}

/// Refuse a directory a runtime is using. Core takes a shared lock on
/// `runtime.sqlite.execution-custody` for each runtime it hosts (a runtime
/// driver or machine), not for an open store, so a process with its stores
/// open and no hosted runtime holds none; the maintenance fence keeps such a
/// process out of the databases while the census copies them. A 0.9.4 Meta
/// runtime holds `meta-runtime.lock`. An exclusive try-lock on each tells. A
/// lock file that does not exist has no holder, and the probe never creates
/// one.
pub(crate) fn exclude_live_runtimes(
    state_dir: &Path,
) -> Result<LiveRuntimeExclusion, NativeMigrateRefusal> {
    let mut custody = state_dir.join(RUNTIME_DB_FILE_NAME).into_os_string();
    custody.push(RUNTIME_EXECUTION_CUSTODY_SUFFIX);
    let mut locks = Vec::new();
    let mut held = Vec::new();
    for path in [
        PathBuf::from(custody),
        state_dir.join(META_RUNTIME_LOCK_FILE_NAME),
    ] {
        let unreadable = |detail: String| NativeMigrateRefusal::RuntimeLockUnreadable {
            file: path.display().to_string(),
            detail,
        };
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(unreadable(error.to_string())),
        };
        match file.try_lock() {
            Ok(()) => {
                locks.push(file);
                held.push(path.clone());
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(NativeMigrateRefusal::RuntimeInUse {
                    file: path.display().to_string(),
                });
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(unreadable(error.to_string())),
        }
    }
    Ok(LiveRuntimeExclusion {
        _locks: locks,
        held,
    })
}

/// The databases the census copies: `runtime.sqlite` and every projection
/// database the shape reads.
fn copied_files(
    shape: GatewayStateShape,
    files: ProjectionFiles,
) -> Vec<(&'static str, &'static str)> {
    // Each file is copied under its canonical name, so the census reads a
    // legacy spelling the way it reads the canonical one.
    let runtime = (RUNTIME_DB_FILE_NAME, RUNTIME_DB_FILE_NAME);
    let sessions = (
        files.sessions.unwrap_or(CANONICAL_SESSIONS_DB_FILE_NAME),
        CANONICAL_SESSIONS_DB_FILE_NAME,
    );
    let continuity = (
        files
            .continuity
            .unwrap_or(CANONICAL_CONTINUITY_DB_FILE_NAME),
        CANONICAL_CONTINUITY_DB_FILE_NAME,
    );
    match shape {
        GatewayStateShape::SessionsProjection => vec![runtime, sessions],
        GatewayStateShape::ContinuityProjection
        | GatewayStateShape::ContinuityProjectionInMemoryMob => vec![runtime, continuity],
        GatewayStateShape::SessionsProjectionWithBindings => vec![runtime, sessions, continuity],
        // The export census copies what it reads itself.
        GatewayStateShape::ContinuityExportNew | GatewayStateShape::ContinuityExportInPlace => {
            Vec::new()
        }
    }
}

/// Copy a database and its write-ahead log into `scratch`. The shared-memory
/// index is not copied; SQLite rebuilds it from the log on first open.
pub(super) fn copy_database(
    state_dir: &Path,
    scratch: &Path,
    file: &str,
    scratch_name: &str,
) -> Result<(), String> {
    for suffix in ["", "-wal"] {
        let source = state_dir.join(format!("{file}{suffix}"));
        if suffix.is_empty() || source.is_file() {
            std::fs::copy(&source, scratch.join(format!("{scratch_name}{suffix}")))
                .map_err(|error| format!("{}: {error}", source.display()))?;
        }
    }
    Ok(())
}

/// What the runtime store copy knows, by session id.
pub(super) struct RuntimeSessions {
    /// Committed WholeBlob sessions.
    committed: BTreeMap<String, Arc<meerkat_core::Session>>,
    /// Sessions whose committed snapshot did not load, with the error.
    unreadable: BTreeMap<String, String>,
    /// Catalog entries, with the lifecycle terminal fact each carries.
    catalog: BTreeMap<String, Option<SessionLifecycleTerminal>>,
    /// The runtime state each catalog entry records, when it records one.
    runtime_states: BTreeMap<String, RuntimeState>,
    /// Sessions whose WholeBlob authority references a missing body, with
    /// the runtime id of each such authority: the session's runtime id, its
    /// legacy alias, or both.
    bodiless: BTreeMap<String, Vec<String>>,
}

impl RuntimeSessions {
    /// A runtime store that knows no session: a new state directory.
    pub(super) fn empty() -> Self {
        Self {
            committed: BTreeMap::new(),
            unreadable: BTreeMap::new(),
            catalog: BTreeMap::new(),
            runtime_states: BTreeMap::new(),
            bodiless: BTreeMap::new(),
        }
    }

    /// The sessions whose WholeBlob authority references a missing body, with
    /// every body-less runtime id each one has.
    pub(super) fn bodiless_runtimes(&self) -> &BTreeMap<String, Vec<String>> {
        &self.bodiless
    }

    pub(super) fn known_session_ids(&self) -> impl Iterator<Item = String> + '_ {
        self.committed
            .keys()
            .chain(self.unreadable.keys())
            .chain(self.catalog.keys())
            .chain(self.bodiless.keys())
            .cloned()
    }

    /// Whether 0.9.3 would treat the session as lifecycle-terminal: its
    /// facade reads any lifecycle terminal fact on the catalog entry.
    fn lifecycle_terminal(&self, session_id: &str) -> Option<SessionLifecycleTerminal> {
        self.catalog.get(session_id).copied().flatten()
    }

    pub(super) fn runtime_state(&self, session_id: &str) -> Option<RuntimeState> {
        self.runtime_states.get(session_id).copied()
    }
}

pub(super) async fn open_runtime_sessions(
    path: &Path,
) -> Result<RuntimeSessions, NativeMigrateRefusal> {
    let unreadable = |detail: String| NativeMigrateRefusal::RuntimeStoreUnreadable { detail };
    // Every runtime with WholeBlob authority, read before the store opens
    // the copy: the conversion activates each of them, listed or not.
    let mut session_ids = BTreeSet::new();
    for runtime_id in whole_blob_runtime_ids(path).map_err(unreadable)? {
        let Some(session_id) = session_of_runtime(&runtime_id) else {
            return Err(NativeMigrateRefusal::ForeignRuntime { runtime_id });
        };
        session_ids.insert(session_id);
    }
    let mut bodiless: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for runtime_id in bodiless_runtime_ids(path).map_err(unreadable)? {
        if let Some(session_id) = session_of_runtime(&runtime_id) {
            bodiless
                .entry(session_id.to_string())
                .or_default()
                .push(runtime_id);
        }
    }
    let store =
        SqliteRuntimeStore::new_whole_blob(path).map_err(|error| unreadable(error.to_string()))?;
    let mut catalog = BTreeMap::new();
    let mut runtime_states = BTreeMap::new();
    for entry in store
        .list_runtime_session_catalog_entries(meerkat_core::SessionFilter::default())
        .await
        .map_err(|error| unreadable(format!("runtime session catalog: {error}")))?
    {
        catalog.insert(entry.session_id().to_string(), entry.lifecycle_terminal());
        if let Some(state) = entry.runtime_state() {
            runtime_states.insert(entry.session_id().to_string(), state);
        }
        session_ids.insert(entry.session_id().clone());
    }
    // One snapshot that does not load is that session's row, not the run's.
    let mut committed = BTreeMap::new();
    let mut unreadable_sessions = BTreeMap::new();
    for session_id in session_ids {
        // Its body is gone: Core's load would only repeat that.
        if bodiless.contains_key(&session_id.to_string()) {
            continue;
        }
        match load_committed(&store, &session_id).await {
            Ok(Some(session)) => {
                committed.insert(session_id.to_string(), session);
            }
            Ok(None) => {}
            Err(detail) => {
                unreadable_sessions.insert(session_id.to_string(), detail);
            }
        }
    }
    Ok(RuntimeSessions {
        committed,
        unreadable: unreadable_sessions,
        catalog,
        runtime_states,
        bodiless,
    })
}

/// The committed WholeBlob session under either of the session's runtime
/// ids, when the store holds one.
async fn load_committed(
    store: &SqliteRuntimeStore,
    session_id: &SessionId,
) -> Result<Option<Arc<meerkat_core::Session>>, String> {
    for runtime_id in [
        LogicalRuntimeId::for_session(session_id),
        LogicalRuntimeId::legacy_session_uuid_alias(session_id),
    ] {
        if let Some(snapshot) = store
            .load_committed_whole_blob_snapshot(&runtime_id)
            .await
            .map_err(|error| format!("committed session {session_id}: {error}"))?
        {
            return Ok(Some(snapshot.session_arc()));
        }
    }
    Ok(None)
}

/// The runtime ids with WholeBlob authority in a runtime store copy.
/// The runtimes whose WholeBlob authority references a body the file does
/// not hold, read straight from Core's tables. A file without the tables
/// has none.
pub(super) fn bodiless_runtime_ids(path: &Path) -> Result<Vec<String>, String> {
    let conn = meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM main.sqlite_master WHERE type = 'table' AND name IN \
             ('runtime_whole_blob_authority', 'runtime_whole_blob_bodies')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("runtime_whole_blob_bodies: {error}"))?;
    if tables != 2 {
        return Ok(Vec::new());
    }
    let mut statement = conn
        .prepare(
            "SELECT authority.runtime_id FROM main.runtime_whole_blob_authority AS authority \
             WHERE NOT EXISTS (SELECT 1 FROM main.runtime_whole_blob_bodies AS bodies \
             WHERE bodies.blob_sha256 = authority.blob_sha256) ORDER BY authority.runtime_id",
        )
        .map_err(|error| format!("runtime_whole_blob_bodies: {error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("runtime_whole_blob_bodies: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("runtime_whole_blob_bodies: {error}"))
}

fn whole_blob_runtime_ids(path: &Path) -> Result<Vec<String>, String> {
    let conn = meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let table: bool = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM main.sqlite_master WHERE type = 'table' AND name = \
             'runtime_whole_blob_authority')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("runtime_whole_blob_authority: {error}"))?;
    if !table {
        return Ok(Vec::new());
    }
    let mut statement = conn
        .prepare("SELECT runtime_id FROM main.runtime_whole_blob_authority ORDER BY runtime_id")
        .map_err(|error| format!("runtime_whole_blob_authority: {error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("runtime_whole_blob_authority: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("runtime_whole_blob_authority: {error}"))
}

/// The session a runtime id names: `rt:session:<id>`, or the bare id of the
/// legacy alias. Any other runtime id names no session.
pub(super) fn session_of_runtime(runtime_id: &str) -> Option<SessionId> {
    let session_id = SessionId::parse(runtime_id.rsplit(':').next()?).ok()?;
    let names_session = LogicalRuntimeId::for_session(&session_id).0 == runtime_id
        || LogicalRuntimeId::legacy_session_uuid_alias(&session_id).0 == runtime_id;
    names_session.then_some(session_id)
}

/// The projection copy: its session store, the sessions it holds a body for,
/// and (for roster layouts) the identities whose continuity record binds each
/// session.
pub(super) struct Projection {
    pub(super) store: Arc<dyn SessionStore>,
    pub(super) session_ids: BTreeSet<String>,
    pub(super) bindings: BTreeMap<String, Vec<String>>,
}

/// Open the projection copies in `scratch` the way `shape` reads them.
async fn open_projection(shape: GatewayStateShape, scratch: &Path) -> Result<Projection, String> {
    let sessions = scratch.join(CANONICAL_SESSIONS_DB_FILE_NAME);
    let continuity = scratch.join(CANONICAL_CONTINUITY_DB_FILE_NAME);
    match shape {
        GatewayStateShape::SessionsProjection => open_sessions_projection(&sessions).await,
        // The sessions store is the projection, read exactly as above; the
        // continuity copy gives only each session's bindings.
        GatewayStateShape::SessionsProjectionWithBindings => {
            let mut projection = open_sessions_projection(&sessions).await?;
            let records = LocalContinuityStore::open(&continuity)
                .map_err(|error| format!("{}: {error}", continuity.display()))?;
            projection.bindings = continuity_bindings(&records).await?;
            Ok(projection)
        }
        GatewayStateShape::ContinuityExportNew | GatewayStateShape::ContinuityExportInPlace => {
            Err("a continuity export is read by the export census".to_owned())
        }
        GatewayStateShape::ContinuityProjection
        | GatewayStateShape::ContinuityProjectionInMemoryMob => {
            // Read before the store opens the copy. The continuity session
            // store adapter does not list, so its sessions are enumerated
            // from the tables that hold their bodies; a session no record
            // binds is otherwise unseen.
            let session_ids = continuity_session_ids(&continuity)?;
            let store = LocalContinuityStore::open(&continuity)
                .map_err(|error| format!("{}: {error}", continuity.display()))?;
            let bindings = continuity_bindings(&store).await?;
            Ok(Projection {
                store: Arc::new(ContinuitySessionStoreAdapter::new(Arc::new(store))),
                session_ids,
                bindings,
            })
        }
    }
}

async fn open_sessions_projection(path: &Path) -> Result<Projection, String> {
    let store = meerkat_store::SqliteSessionStore::open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let session_ids = store
        .list(meerkat_core::SessionFilter::default())
        .await
        .map_err(|error| format!("projection sessions: {error}"))?
        .into_iter()
        .map(|meta| meta.id.to_string())
        .collect();
    Ok(Projection {
        store: Arc::new(store),
        session_ids,
        bindings: BTreeMap::new(),
    })
}

/// The identities whose continuity record binds each session.
async fn continuity_bindings(
    store: &LocalContinuityStore,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut bindings: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (identity, session_id) in store
        .list_session_bindings()
        .await
        .map_err(|error| format!("continuity records: {error}"))?
    {
        bindings
            .entry(session_id.to_string())
            .or_default()
            .push(identity.to_string());
    }
    Ok(bindings)
}

/// The session ids a continuity copy holds a body for: a head row or a
/// snapshot row. Strand rows without either are rows no head adopted (a
/// write the crash window left), which 0.9.3 never serves.
fn continuity_session_ids(path: &Path) -> Result<BTreeSet<String>, String> {
    let conn = meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let mut session_ids = BTreeSet::new();
    for table in CONTINUITY_SESSION_BODY_TABLES {
        let present: bool = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM main.sqlite_master WHERE type = 'table' AND name = \
                 ?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(|error| format!("{table}: {error}"))?;
        if !present {
            continue;
        }
        let mut statement = conn
            .prepare(&format!("SELECT DISTINCT session_id FROM main.{table}"))
            .map_err(|error| format!("{table}: {error}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("{table}: {error}"))?;
        for session_id in rows {
            session_ids.insert(session_id.map_err(|error| format!("{table}: {error}"))?);
        }
    }
    Ok(session_ids)
}

/// Whether the sessions database at `path` holds a row a release reads: a
/// row of any table but [`INERT_SESSIONS_TABLES`]. A file that holds none
/// carries no session, and retiring it loses nothing a release reads.
fn sessions_database_holds_rows(path: &Path) -> Result<bool, String> {
    let conn = meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let tables = {
        let mut statement = conn
            .prepare(
                "SELECT name FROM main.sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'",
            )
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("{}: {error}", path.display()))?;
        names
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("{}: {error}", path.display()))?
    };
    for table in tables {
        if INERT_SESSIONS_TABLES.contains(&table.as_str()) {
            continue;
        }
        let holds: bool = conn
            .query_row(
                &format!(
                    "SELECT EXISTS (SELECT 1 FROM main.\"{}\")",
                    table.replace('"', "\"\"")
                ),
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("{table}: {error}"))?;
        if holds {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The first of `runtime_ids` that is not `session_id`'s own runtime id
/// (every one of them when the session id does not parse).
fn foreign_runtime_id<'a>(session_id: &str, runtime_ids: &'a [String]) -> Option<&'a str> {
    let own = SessionId::parse(session_id)
        .ok()
        .map(|session| LogicalRuntimeId::for_session(&session).0);
    runtime_ids
        .iter()
        .map(String::as_str)
        .find(|runtime_id| own.as_deref() != Some(*runtime_id))
}

/// Classify one session. `bound` says whether a continuity record binds it.
pub(super) async fn classify_session(
    session_id: &str,
    runtime: &RuntimeSessions,
    in_projection: bool,
    projection: &Arc<dyn SessionStore>,
    shape: GatewayStateShape,
    bound: bool,
) -> (SessionCensusClass, String) {
    if let Some(runtime_ids) = runtime.bodiless.get(session_id) {
        // Core reads and clears WholeBlob authority only under the session's
        // own runtime id, and no released Core writes one under another id
        // (the legacy alias, the bare session id). `--discard-bodyless` could
        // not discard such an authority, so the census refuses it here,
        // before `--apply` writes anything.
        if let Some(foreign) = foreign_runtime_id(session_id, runtime_ids) {
            return (
                SessionCensusClass::Unreadable,
                format!(
                    "runtime.sqlite holds the session's WholeBlob authority under runtime \
                     {foreign}, not under the session's own runtime id, and it references a \
                     body the file does not hold; Core reads and clears authority only under \
                     the session's runtime id and no released Core writes one elsewhere, so \
                     --discard-bodyless cannot discard it"
                ),
            );
        }
        return (
            SessionCensusClass::BodilessRuntime,
            format!(
                "runtime.sqlite's WholeBlob authority references a body the file does not hold \
                 (runtime {}); the projection {}",
                runtime_ids.join(", "),
                if in_projection {
                    "holds a copy"
                } else {
                    "holds no copy"
                }
            ),
        );
    }
    if let Some(detail) = runtime.unreadable.get(session_id) {
        return (SessionCensusClass::Unreadable, detail.clone());
    }
    let catalog = runtime.catalog.get(session_id);
    // The copy `--apply` writes for an admitted row: Core's committed copy,
    // or the projection copy an import takes (held here).
    let mut reminted = None;
    let (class, detail, written) = match (runtime.committed.get(session_id), catalog) {
        (None, Some(&lifecycle)) => (
            SessionCensusClass::RuntimeLifecycleOnly,
            format!(
                "runtime.sqlite has a catalog entry (lifecycle terminal fact: {}) and no committed \
                 snapshot; the projection {}",
                lifecycle.map_or("none".to_owned(), |terminal| format!("{terminal:?}")),
                if in_projection {
                    "holds a copy"
                } else {
                    "holds no copy"
                }
            ),
            None,
        ),
        (None, None) => match remint_copy(session_id, projection).await {
            Ok(copy) => {
                let detail = format!(
                    "runtime.sqlite knows nothing of the session; the projection copy of {} \
                     messages is the seed 0.9.3 re-mints ({})",
                    copy.session.messages().len(),
                    copy.graph
                );
                let copy: &RemintCopy = reminted.insert(copy);
                (
                    SessionCensusClass::RuntimeMissing,
                    detail,
                    Some(&copy.session),
                )
            }
            Err(detail) => (SessionCensusClass::Unreadable, detail, None),
        },
        (Some(successor), _) if !in_projection => (
            SessionCensusClass::ProjectionMissing,
            format!(
                "the projection holds no copy; runtime.sqlite holds {} messages",
                successor.messages().len()
            ),
            Some(successor.as_ref()),
        ),
        (Some(successor), _) => {
            let (class, detail) = match compare(session_id, successor, projection).await {
                Ok((ProvenRelation::InverseAppendSuffix { suffix_rows }, detail)) => suffix_class(
                    format!("{detail}; {suffix_rows} projection rows past the rewrite's parent"),
                    runtime.lifecycle_terminal(session_id),
                    shape,
                    bound,
                ),
                Ok((relation, detail)) => (relation.class(), detail),
                Err(detail) => (SessionCensusClass::Unreadable, detail),
            };
            (class, detail, Some(successor.as_ref()))
        }
    };
    // Every admitted class leaves through here, so none skips Core's layout
    // of the session `--apply` writes.
    if !class.admitted() {
        return (class, detail);
    }
    match written {
        Some(session) => layout_checked(class, detail, session),
        // No admitted arm lacks the copy; refuse rather than admit unchecked.
        None => (
            SessionCensusClass::LayoutRefused,
            format!("{detail}; the census holds no copy of the session to lay out"),
        ),
    }
}

/// The scratch state directory a census imports each `RuntimeMissing` copy
/// into for trial.
pub(super) const IMPORT_TRIAL_DIR_NAME: &str = "import-trial";

/// What the import will do with each `RuntimeMissing` row, proved before
/// `--apply` writes anything.
///
/// A row the session store already holds, read on the census's copy of
/// `runtime.sqlite` (a session store opened there creates its schema in the
/// copy alone), is compared the way the import's I2 compares it: an equal
/// row (the same transcript and model-routing control history) is kept, and
/// a different one would refuse the import's create-only write, so the row
/// is refused here as [`SessionCensusClass::ImportRowConflict`].
///
/// Every other row is imported for trial: the import's own
/// [`super::import::import_session`] (I1-I4) runs on the copy, against the
/// native session store, runtime store and blob store of a scratch state
/// directory of its own ([`IMPORT_TRIAL_DIR_NAME`] in `scratch`, never the
/// census's copy of `runtime.sqlite`). Core's session store lays a copy out
/// as head-canonical rows only at I3, with checks the census's layout check
/// does not run, so a copy the import refuses there, or at any other step,
/// is refused here as [`SessionCensusClass::LayoutRefused`] with the
/// import's refusal: one dry run names every session `--apply` would stop
/// on. `runtime_copy` is `None` for a new directory, which holds no row yet.
pub(super) async fn predict_import_rows(
    runtime_copy: Option<&Path>,
    scratch: &Path,
    projection: &Arc<dyn SessionStore>,
    rows: &mut [SessionCensusRow],
) -> Result<(), String> {
    if !rows
        .iter()
        .any(|row| row.class == SessionCensusClass::RuntimeMissing)
    {
        return Ok(());
    }
    let store = runtime_copy
        .map(|path| {
            meerkat_store::SqliteSessionStore::open(path.to_path_buf())
                .map_err(|error| format!("session store on {}: {error}", path.display()))
        })
        .transpose()?;
    let trial_dir = scratch.join(IMPORT_TRIAL_DIR_NAME);
    std::fs::create_dir_all(&trial_dir)
        .map_err(|error| format!("trial directory {}: {error}", trial_dir.display()))?;
    let (sessions, runtime) = super::import::open_native_pair(&trial_dir)?;
    let trial = super::import::NativeImportStores {
        sessions,
        runtime,
        blobs: Arc::new(super::import::RealmBlobs::open(&trial_dir)?),
    };
    for row in rows
        .iter_mut()
        .filter(|row| row.class == SessionCensusClass::RuntimeMissing)
    {
        let id = SessionId::parse(&row.session_id)
            .map_err(|error| format!("session {}: {error}", row.session_id))?;
        let existing = match &store {
            Some(store) => store
                .load(&id)
                .await
                .map_err(|error| format!("session store row of {id}: {error}"))?,
            None => None,
        };
        let Some(existing) = existing else {
            let copy = remint_copy(&row.session_id, projection).await?;
            match super::import::import_session(&trial, &copy.session).await {
                Ok(_) => row
                    .detail
                    .push_str("; the session store holds no row for it, so the import creates one"),
                Err(refusal) => {
                    *row = SessionCensusRow::new(
                        row.session_id.clone(),
                        SessionCensusClass::LayoutRefused,
                        row.identities.clone(),
                        format!(
                            "{}; a trial import into a scratch directory refuses it: {refusal}",
                            row.detail
                        ),
                        None,
                        false,
                    );
                }
            }
            continue;
        };
        let copy = remint_copy(&row.session_id, projection).await?;
        match super::import::row_difference(&existing, &copy.session) {
            None => row
                .detail
                .push_str("; the session store already holds an equal row, which the import keeps"),
            Some(difference) => {
                *row = SessionCensusRow::new(
                    row.session_id.clone(),
                    SessionCensusClass::ImportRowConflict,
                    row.identities.clone(),
                    format!(
                        "{}; the session store already holds another row: {difference}",
                        row.detail
                    ),
                    None,
                    false,
                );
            }
        }
    }
    Ok(())
}

/// The class of a body-less runtime the operator names with
/// `--discard-bodyless`. Its authority is discarded; when the projection
/// holds a copy, that copy is then the session's only one, so `--apply`
/// imports it as a session only the projection holds: the class a rerun
/// after the discard reads, so the straight path and a restart agree. The
/// copy is checked as a `RuntimeMissing` row's is.
pub(super) async fn discarded_bodiless(
    session_id: &str,
    in_projection: bool,
    projection: &Arc<dyn SessionStore>,
    detail: String,
) -> (SessionCensusClass, String) {
    if !in_projection {
        return (SessionCensusClass::BodilessRuntimeDiscarded, detail);
    }
    match remint_copy(session_id, projection).await {
        Ok(copy) => {
            let detail = format!(
                "{detail}; --discard-bodyless discards that authority, and --apply then imports \
                 the projection copy of {} messages ({})",
                copy.session.messages().len(),
                copy.graph
            );
            layout_checked(SessionCensusClass::RuntimeMissing, detail, &copy.session)
        }
        Err(reason) => (
            SessionCensusClass::Unreadable,
            format!(
                "{detail}; --discard-bodyless would leave the projection copy as the session's \
                 only one, and it does not load: {reason}"
            ),
        ),
    }
}

/// The class of an inverse-append suffix. 0.9.3 keeps the rows past the
/// rewrite's parent through a repair whose first step writes the session, so
/// they are lost only where 0.9.3 never serves them: on a session with a
/// lifecycle terminal fact (it drops them itself), and, in a continuity
/// layout, on a session no record binds (its facade refuses that write: it
/// writes a head-canonical session only for an owner a binding record
/// registers). "Unbound" reads `continuity_records` only; a mob member
/// binding that still names the session is the identity census's to refuse.
/// Where the projection is `sessions.sqlite3`, with or without bindings,
/// every such row stays refused: that store writes without an owner.
fn suffix_class(
    detail: String,
    terminal: Option<SessionLifecycleTerminal>,
    shape: GatewayStateShape,
    bound: bool,
) -> (SessionCensusClass, String) {
    let continuity = matches!(
        shape,
        GatewayStateShape::ContinuityProjection
            | GatewayStateShape::ContinuityProjectionInMemoryMob
    );
    match terminal {
        Some(terminal) => (
            SessionCensusClass::ProjectionSuffixTerminal,
            format!("{detail}; lifecycle terminal fact {terminal:?}"),
        ),
        None if continuity && !bound => (
            SessionCensusClass::ProjectionSuffixUnbound,
            format!("{detail}; no continuity record binds the session"),
        ),
        None => (SessionCensusClass::ProjectionSuffix, detail),
    }
}

/// Keep an admitted class only when Core can lay the session `--apply`
/// writes out as head-canonical rows: the conversion seeds every committed
/// body through that layout, and an import crosses it. Core's own pure
/// layout of the retained history decides (a graph re-anchored past its
/// oldest rewrites lays out from its retired base, as the conversion seeds
/// it), so the census refuses exactly what the apply would, and before any
/// write. The session laid out is the one `--apply` writes: every image the
/// session holds inline, its compacted history included, moved to a blob
/// reference by Core's own pure transform (inline video stays row content).
/// The detail says how many images move to the blob store.
pub(super) fn layout_checked(
    class: SessionCensusClass,
    detail: String,
    session: &meerkat_core::Session,
) -> (SessionCensusClass, String) {
    let (detail, layout) = match session.with_inline_images_externalized() {
        Ok(None) => (detail, head_canonical_layout(session)),
        Ok(Some(moved)) => (
            format!("{detail}; {}", images_moved(moved.payloads.len())),
            head_canonical_layout(&moved.session),
        ),
        Err(error) => (
            detail,
            Err(format!(
                "its inline images cannot move to the blob store: {error}"
            )),
        ),
    };
    match layout {
        Ok(()) => (class, detail),
        Err(reason) => (
            SessionCensusClass::LayoutRefused,
            format!("{detail}; Core cannot lay the session out as head-canonical rows: {reason}"),
        ),
    }
}

fn head_canonical_layout(session: &meerkat_core::Session) -> Result<(), String> {
    let history = session
        .validated_transcript_history_state()
        .map_err(|error| error.to_string())?;
    meerkat_core::strand_layout_for_retained_history(session, history.as_ref())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn images_moved(count: usize) -> String {
    if count == 1 {
        "1 inline image moves to the blob store".to_owned()
    } else {
        format!("{count} inline images move to the blob store")
    }
}

/// The seed 0.9.3 mints from the projection for a runtime its store knows
/// nothing of, built the way its facade builds it: the importing load of the
/// listed id, the rewrite graph hydrated from the store's adopted records
/// (a graph that does not prove refuses the seed), and the persisted
/// encoding. Records a legacy shape cannot reconstruct leave the seed slim,
/// as in 0.9.3.
pub(super) async fn remint_copy(
    session_id: &str,
    projection: &Arc<dyn SessionStore>,
) -> Result<RemintCopy, String> {
    let id = SessionId::parse(session_id)
        .map_err(|error| format!("the projection names an invalid session id: {error}"))?;
    let mut session = projection
        .load(&id)
        .await
        .map_err(|error| format!("projection copy of {session_id}: {error}"))?
        .ok_or_else(|| {
            "runtime.sqlite knows nothing of the session and the projection holds no body for it"
                .to_owned()
        })?;
    if session.id() != &id {
        return Err(format!(
            "the projection copy listed as {session_id} carries the id {}",
            session.id()
        ));
    }
    let mut graph = RemintGraph::NoRecords;
    if let Some(incremental) = Arc::clone(projection).as_incremental() {
        match incremental.load_rewrites(&id).await {
            Ok(records) if records.is_empty() => {}
            Ok(records) => {
                let count = records.len();
                // A re-anchored graph's store answers only its retained
                // records, which prove nothing without the retired prefix.
                // 0.9.3's stores refused to lay such a graph out at all, so
                // 0.9.3 seeded the session slim; the census does the same.
                let retained_only = records
                    .iter()
                    .map(|record| record.commit.rewrite_generation)
                    .min()
                    .is_some_and(|first| first > 1);
                if retained_only {
                    graph = RemintGraph::SlimLegacy {
                        detail: format!(
                            "the store holds only the {count} retained rewrite records of a \
                             re-anchored graph"
                        ),
                    };
                } else {
                    let validated =
                        meerkat_core::ValidatedTranscriptHistory::from_rewrite_records_with_proved(
                            records, None,
                        )
                        .map_err(|error| format!("projection rewrite graph proof: {error}"))?;
                    if let Some(validated) = validated {
                        session
                            .install_validated_audited_transcript_history_preserving_live(validated)
                            .map_err(|error| {
                                format!("projection rewrite graph install: {error}")
                            })?;
                        graph = RemintGraph::Hydrated { records: count };
                    }
                }
            }
            Err(error) => {
                graph = RemintGraph::SlimLegacy {
                    detail: error.to_string(),
                };
            }
        }
    }
    session
        .validated_transcript_history_state()
        .map_err(|error| format!("projection copy rewrite history: {error}"))?;
    session
        .to_persisted_bytes()
        .map_err(|error| format!("projection copy encoding: {error}"))?;
    Ok(RemintCopy { session, graph })
}

/// A projection copy that passes the 0.9.3 re-mint steps.
pub(super) struct RemintCopy {
    pub(super) session: meerkat_core::Session,
    graph: RemintGraph,
}

/// How the re-mint seed got its rewrite graph.
enum RemintGraph {
    /// The store holds no adopted rewrite records for the session.
    NoRecords,
    /// The graph was rebuilt from this many adopted records and proved.
    Hydrated { records: usize },
    /// The records did not reconstruct (a legacy shape); 0.9.3 seeds slim.
    SlimLegacy { detail: String },
}

impl std::fmt::Display for RemintGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRecords => f.write_str("no rewrite records"),
            Self::Hydrated { records } => {
                write!(f, "rewrite graph proved from {records} records")
            }
            Self::SlimLegacy { detail } => {
                write!(
                    f,
                    "rewrite records did not reconstruct, seeded slim: {detail}"
                )
            }
        }
    }
}

/// What the provers establish between a committed session and its
/// projection copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProvenRelation {
    Equal,
    Behind,
    /// The projection is a committed rewrite's parent plus `suffix_rows`
    /// rows the runtime never committed.
    InverseAppendSuffix {
        suffix_rows: usize,
    },
    Ahead,
    Divergent,
}

impl ProvenRelation {
    fn class(self) -> SessionCensusClass {
        match self {
            Self::Equal => SessionCensusClass::Equal,
            Self::Behind => SessionCensusClass::ProjectionBehind,
            Self::InverseAppendSuffix { .. } => SessionCensusClass::ProjectionSuffix,
            Self::Ahead => SessionCensusClass::ProjectionAhead,
            Self::Divergent => SessionCensusClass::Divergent,
        }
    }
}

async fn compare(
    session_id: &str,
    successor: &meerkat_core::Session,
    projection: &Arc<dyn SessionStore>,
) -> Result<(ProvenRelation, String), String> {
    let id = successor.id().clone();
    let sealed = successor
        .validated_transcript_history_state()
        .map_err(|error| format!("committed rewrite history: {error}"))?;
    let rewrites = sealed
        .as_ref()
        .is_some_and(|sealed| sealed.commit_count() != 0);
    if let Some(sealed) = sealed.as_ref().filter(|_| rewrites) {
        match relation::durable_head_relation(projection, successor, sealed)
            .await
            .map_err(|error| error.to_string())?
        {
            DurableHeadRelation::AtHead => {
                return Ok((
                    ProvenRelation::Equal,
                    "the projection head is at the committed head".to_owned(),
                ));
            }
            DurableHeadRelation::CommittedAppendPrefix => {
                return Ok((
                    ProvenRelation::Behind,
                    "the projection head is the committed transcript's prefix past its last \
                     rewrite"
                        .to_owned(),
                ));
            }
            DurableHeadRelation::Unproven => {}
        }
    }
    let Some(durable) = projection
        .load(&id)
        .await
        .map_err(|error| format!("projection copy of {session_id}: {error}"))?
    else {
        return Err(format!(
            "the projection lists the session but holds no body for it; runtime.sqlite holds {} \
             messages",
            successor.messages().len()
        ));
    };
    let durable_revision = durable
        .transcript_revision()
        .map_err(|error| format!("projection revision: {error}"))?;
    let committed_revision = successor
        .transcript_revision()
        .map_err(|error| format!("committed revision: {error}"))?;
    let committed_generation = successor
        .transcript_rewrite_generation()
        .map_err(|error| format!("committed rewrite generation: {error}"))?;
    let durable_generation =
        match relation::durable_head_generation(projection, &id, "storage-migrate-native census")
            .await
            .map_err(|error| error.to_string())?
        {
            Some(generation) => generation,
            None => durable
                .transcript_rewrite_generation()
                .map_err(|error| format!("projection rewrite generation: {error}"))?,
        };
    let durable_len = durable.messages().len();
    let committed_len = successor.messages().len();
    let counts = format!("projection {durable_len} messages, runtime {committed_len}");
    if durable_revision == committed_revision && durable_generation == committed_generation {
        return Ok((ProvenRelation::Equal, counts));
    }
    // Past the committed head with nothing rewritten in between: the
    // projection extends the committed transcript itself.
    if durable_generation == committed_generation
        && durable_len > committed_len
        && durable
            .transcript_prefix_digest(committed_len)
            .map_err(|error| format!("projection prefix digest: {error}"))?
            == committed_revision
    {
        return Ok((
            ProvenRelation::Ahead,
            format!(
                "{counts}; {} projection messages past the committed head",
                durable_len - committed_len
            ),
        ));
    }
    let Some(sealed) = sealed.as_ref().filter(|_| rewrites) else {
        // No rewrite: the projection must be a plain prefix of the
        // committed transcript.
        if durable_len <= committed_len
            && successor
                .transcript_prefix_digest(durable_len)
                .map_err(|error| format!("committed prefix digest: {error}"))?
                == durable_revision
        {
            return Ok((ProvenRelation::Behind, counts));
        }
        return Ok((ProvenRelation::Divergent, counts));
    };
    if relation::durable_is_committed_append_prefix(
        sealed,
        &durable,
        &durable_revision,
        durable_generation,
        committed_generation,
        successor,
    )
    .map_err(|error| error.to_string())?
    {
        return Ok((
            ProvenRelation::Behind,
            format!("{counts}; append prefix past the last rewrite"),
        ));
    }
    if meerkat_core::session_store::find_transcript_rewrite_commit_chain_extending_session(
        sealed,
        &durable,
        sealed.state().head(),
    )
    .map_err(|error| format!("rewrite chain walk: {error}"))?
    .is_some()
    {
        return Ok((
            ProvenRelation::Behind,
            format!("{counts}; the committed rewrite chain extends the projection"),
        ));
    }
    if let Some(chain) = relation::inverse_append_replay_chain(sealed, &durable)
        .map_err(|error| error.to_string())?
        && let Some(first) = chain.first()
    {
        let suffix_rows = durable_len - first.messages_before;
        if suffix_rows == 0 {
            return Ok((
                ProvenRelation::Behind,
                format!(
                    "{counts}; the projection is the parent of committed rewrite generation {}",
                    first.rewrite_generation
                ),
            ));
        }
        return Ok((
            ProvenRelation::InverseAppendSuffix { suffix_rows },
            format!(
                "{counts}; the projection extends the parent of committed rewrite generation {}",
                first.rewrite_generation
            ),
        ));
    }
    if relation::durable_behind_prefix_chain(successor, sealed, &durable)
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Ok((
            ProvenRelation::Behind,
            format!("{counts}; the projection is a prefix of a committed rewrite's parent"),
        ));
    }
    Ok((ProvenRelation::Divergent, counts))
}

/// The durable mob store a state directory's mob lives in, found the way
/// the census finds it with no `--mob-store`: the directory's
/// `event_log.sqlite3`, or the one other store whose composition manifest
/// sits beside it (an embedded application's store under another name).
/// `None` for in-memory mob storage; an error when the directory does not
/// say which store holds the mob.
pub(crate) fn state_dir_mob_store(state_dir: &Path) -> Result<Option<PathBuf>, String> {
    match resolve_mob_storage(state_dir, None) {
        Ok(CensusMobStore::Durable { path }) => Ok(Some(path)),
        Ok(CensusMobStore::InMemory) => Ok(None),
        Err(NativeMigrateRefusal::MobStoreUnnamed { manifests }) if manifests.len() == 1 => {
            Ok(manifests[0]
                .strip_suffix(COMPOSITION_MANIFEST_SUFFIX)
                .map(|store| state_dir.join(store)))
        }
        Err(refusal) => Err(format!("{refusal:?}")),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
pub(super) mod tests {
    use super::*;

    /// The doctor finds a directory's mob store the way the census does.
    #[test]
    fn a_state_dir_mob_store_is_the_store_the_census_reads() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(state_dir_mob_store(dir.path()).unwrap(), None);

        std::fs::write(dir.path().join("mob.sqlite3"), b"").unwrap();
        std::fs::write(dir.path().join("mob.sqlite3.composition.json"), b"{}").unwrap();
        assert_eq!(
            state_dir_mob_store(dir.path()).unwrap(),
            Some(dir.path().join("mob.sqlite3"))
        );

        let gateway = tempfile::tempdir().unwrap();
        std::fs::write(gateway.path().join(EVENT_LOG_DB_FILE_NAME), b"").unwrap();
        assert_eq!(
            state_dir_mob_store(gateway.path()).unwrap(),
            Some(gateway.path().join(EVENT_LOG_DB_FILE_NAME))
        );
    }
    use crate::storage_migrate_native::relation::tests::commit_test_compaction;

    fn message(text: &str) -> meerkat_core::Message {
        meerkat_core::Message::User(meerkat_core::types::UserMessage::text(text))
    }

    fn extended(session: &meerkat_core::Session, texts: &[&str]) -> meerkat_core::Session {
        let mut session = session.clone();
        for text in texts {
            session.push(message(text));
        }
        session
    }

    async fn projection_of(sessions: &[&meerkat_core::Session]) -> Arc<dyn SessionStore> {
        let store = meerkat_store::MemoryStore::new();
        for session in sessions {
            store.save(session).await.unwrap();
        }
        Arc::new(store)
    }

    /// A projection that holds bodies exactly as given, rewrite history
    /// included, with no head rows: the provers decide from the bodies.
    /// (`MemoryStore` refuses a first save that carries a rewrite history.)
    #[derive(Default)]
    struct BodiesOnly(std::sync::Mutex<Vec<meerkat_core::Session>>);

    #[async_trait::async_trait]
    impl SessionStore for BodiesOnly {
        async fn save(
            &self,
            session: &meerkat_core::Session,
        ) -> Result<(), meerkat_core::SessionStoreError> {
            let mut bodies = self.0.lock().unwrap();
            bodies.retain(|body| body.id() != session.id());
            bodies.push(session.clone());
            Ok(())
        }

        async fn load(
            &self,
            id: &SessionId,
        ) -> Result<Option<meerkat_core::Session>, meerkat_core::SessionStoreError> {
            let bodies = self.0.lock().unwrap();
            Ok(bodies.iter().find(|body| body.id() == id).cloned())
        }

        async fn list(
            &self,
            _filter: meerkat_core::SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, meerkat_core::SessionStoreError> {
            let bodies = self.0.lock().unwrap();
            Ok(bodies.iter().map(meerkat_core::SessionMeta::from).collect())
        }

        async fn delete(&self, id: &SessionId) -> Result<(), meerkat_core::SessionStoreError> {
            self.0.lock().unwrap().retain(|body| body.id() != id);
            Ok(())
        }

        async fn delete_if_current_revision(
            &self,
            id: &SessionId,
            _expected_current_revision: &str,
        ) -> Result<bool, meerkat_core::SessionStoreError> {
            let mut bodies = self.0.lock().unwrap();
            let before = bodies.len();
            bodies.retain(|body| body.id() != id);
            Ok(bodies.len() != before)
        }
    }

    async fn relation(
        committed: &meerkat_core::Session,
        projection: &meerkat_core::Session,
    ) -> ProvenRelation {
        let bodies = BodiesOnly::default();
        bodies.save(projection).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(bodies);
        compare(&committed.id().to_string(), committed, &store)
            .await
            .unwrap()
            .0
    }

    /// The catalog entry a runtime store copy holds for one session.
    #[derive(Debug, Clone, Copy)]
    enum Catalog {
        Absent,
        Entry(Option<SessionLifecycleTerminal>),
    }

    fn runtime_with(
        committed: Option<&meerkat_core::Session>,
        catalog: Catalog,
        session_id: &str,
    ) -> RuntimeSessions {
        RuntimeSessions {
            committed: committed
                .map(|session| (session_id.to_owned(), Arc::new(session.clone())))
                .into_iter()
                .collect(),
            unreadable: BTreeMap::new(),
            catalog: match catalog {
                Catalog::Absent => BTreeMap::new(),
                Catalog::Entry(terminal) => BTreeMap::from([(session_id.to_owned(), terminal)]),
            },
            runtime_states: BTreeMap::new(),
            bodiless: BTreeMap::new(),
        }
    }

    /// Classify a session of a sessions-projection directory, where no
    /// continuity record binds anything.
    async fn classify(
        session_id: &str,
        runtime: &RuntimeSessions,
        in_projection: bool,
        projection: &Arc<dyn SessionStore>,
    ) -> (SessionCensusClass, String) {
        classify_session(
            session_id,
            runtime,
            in_projection,
            projection,
            GatewayStateShape::SessionsProjection,
            false,
        )
        .await
    }

    /// `[opening, second]`, its parent `[opening, second, third]`, and the
    /// committed session that compacted the parent into one summary.
    fn rewritten() -> (
        meerkat_core::Session,
        meerkat_core::Session,
        meerkat_core::Session,
    ) {
        let base = extended(&meerkat_core::Session::new(), &["opening", "second"]);
        let parent = extended(&base, &["third"]);
        let mut committed = parent.clone();
        commit_test_compaction(&mut committed, 3, "summary");
        (base, parent, committed)
    }

    #[tokio::test]
    async fn plain_transcripts_relate_by_prefix_only() {
        let base = extended(&meerkat_core::Session::new(), &["opening", "second"]);
        let committed = extended(&base, &["third"]);
        assert_eq!(
            relation(&committed, &committed).await,
            ProvenRelation::Equal
        );
        assert_eq!(relation(&committed, &base).await, ProvenRelation::Behind);
        assert_eq!(
            relation(&committed, &extended(&committed, &["unacknowledged"])).await,
            ProvenRelation::Ahead
        );
        assert_eq!(
            relation(&committed, &extended(&base, &["other third"])).await,
            ProvenRelation::Divergent
        );
    }

    #[tokio::test]
    async fn rewritten_transcripts_relate_through_the_provers() {
        let (base, parent, committed) = rewritten();
        assert_eq!(
            relation(&committed, &committed).await,
            ProvenRelation::Equal
        );
        // A strict prefix of the rewrite's parent (durable-behind prover).
        assert_eq!(relation(&committed, &base).await, ProvenRelation::Behind);
        // The parent itself: behind, never a zero-row suffix.
        assert_eq!(relation(&committed, &parent).await, ProvenRelation::Behind);
        // The parent plus rows the runtime never committed.
        assert_eq!(
            relation(&committed, &extended(&parent, &["fourth", "fifth"])).await,
            ProvenRelation::InverseAppendSuffix { suffix_rows: 2 }
        );
        // Past the committed head with nothing rewritten in between.
        assert_eq!(
            relation(&committed, &extended(&committed, &["after"])).await,
            ProvenRelation::Ahead
        );
        // Behind the committed head past the rewrite (append-prefix prover).
        let appended = extended(&committed, &["after", "more"]);
        assert_eq!(
            relation(&appended, &extended(&committed, &["after"])).await,
            ProvenRelation::Behind
        );
        assert_eq!(
            relation(&committed, &extended(&base, &["other third"])).await,
            ProvenRelation::Divergent
        );
    }

    /// 0.9.3 keeps an inverse-append suffix (rebased over the rewrite) unless
    /// the catalog entry carries a lifecycle terminal fact (its facade reads
    /// any fact, `Active` included) or, in a continuity layout, no record
    /// binds the session (its facade then refuses the repair's first write).
    /// A sessions-projection directory has no records and its store writes
    /// without an owner, so its suffix rows stay refused.
    #[tokio::test]
    async fn an_inverse_append_suffix_is_admitted_only_where_0_9_3_never_serves_its_rows() {
        use GatewayStateShape::{
            ContinuityProjection as Continuity, ContinuityProjectionInMemoryMob as InMemoryMob,
            SessionsProjection as Sessions, SessionsProjectionWithBindings as WithBindings,
        };
        use SessionCensusClass::{
            ProjectionSuffix as Suffix, ProjectionSuffixTerminal as SuffixTerminal,
            ProjectionSuffixUnbound as SuffixUnbound,
        };
        use SessionLifecycleTerminal::{Active, Archived};
        let (_, parent, committed) = rewritten();
        let projection = projection_of(&[&extended(&parent, &["fourth"])]).await;
        let id = committed.id().to_string();
        let no_fact = Catalog::Entry(None);
        let fact = |terminal| Catalog::Entry(Some(terminal));
        for (catalog, shape, bound, expected) in [
            // No terminal fact: refused unless unbound in a continuity layout.
            (Catalog::Absent, Sessions, false, Suffix),
            (no_fact, Sessions, false, Suffix),
            (no_fact, Continuity, true, Suffix),
            (no_fact, InMemoryMob, true, Suffix),
            (no_fact, WithBindings, false, Suffix),
            (no_fact, WithBindings, true, Suffix),
            (Catalog::Absent, Continuity, false, SuffixUnbound),
            (no_fact, Continuity, false, SuffixUnbound),
            (no_fact, InMemoryMob, false, SuffixUnbound),
            // A terminal fact wins in every layout, bound or not.
            (fact(Archived), Sessions, false, SuffixTerminal),
            (fact(Active), Sessions, false, SuffixTerminal),
            (fact(Archived), Continuity, false, SuffixTerminal),
            (fact(Active), Continuity, true, SuffixTerminal),
            (fact(Archived), InMemoryMob, false, SuffixTerminal),
        ] {
            let runtime = runtime_with(Some(&committed), catalog, &id);
            let (class, detail) =
                classify_session(&id, &runtime, true, &projection, shape, bound).await;
            let case = format!("catalog {catalog:?}, {shape:?}, bound {bound}: {detail}");
            assert_eq!(class, expected, "{case}");
            assert!(
                detail.contains("1 projection rows past the rewrite's parent"),
                "{case}"
            );
            assert_eq!(
                detail.ends_with("; no continuity record binds the session"),
                expected == SuffixUnbound,
                "{case}"
            );
        }
        assert!(!Suffix.admitted());
        assert!(SuffixTerminal.admitted());
        assert!(SuffixUnbound.admitted());
    }

    /// Only a session `runtime.sqlite` knows nothing of is imported, and only
    /// when its projection copy passes the re-mint steps.
    #[tokio::test]
    async fn only_a_session_the_runtime_store_knows_nothing_of_is_imported() {
        let session = extended(&meerkat_core::Session::new(), &["opening", "second"]);
        let id = session.id().to_string();
        let projection = projection_of(&[&session]).await;

        let (class, detail) = classify(
            &id,
            &runtime_with(None, Catalog::Absent, &id),
            true,
            &projection,
        )
        .await;
        assert_eq!(class, SessionCensusClass::RuntimeMissing, "{detail}");
        assert!(class.admitted());

        // A catalog entry without a snapshot is a lifecycle statement, not a
        // cold store: 0.9.3 does not re-mint it.
        for terminal in [None, Some(SessionLifecycleTerminal::Archived)] {
            let runtime = runtime_with(None, Catalog::Entry(terminal), &id);
            let (class, detail) = classify(&id, &runtime, true, &projection).await;
            assert_eq!(class, SessionCensusClass::RuntimeLifecycleOnly, "{detail}");
            assert!(!class.admitted());
        }

        // Bound by a continuity record, with no body anywhere.
        let unbacked = SessionId::new().to_string();
        let (class, detail) = classify(
            &unbacked,
            &runtime_with(None, Catalog::Absent, &unbacked),
            false,
            &projection,
        )
        .await;
        assert_eq!(class, SessionCensusClass::Unreadable);
        assert!(detail.contains("holds no body"), "{detail}");
    }

    /// A session only runtime.sqlite holds keeps Core's copy (a record that
    /// binds it without a body is no copy either); a session whose body the
    /// projection lists but cannot load cannot be compared and refuses.
    #[tokio::test]
    async fn a_copy_only_the_runtime_store_holds_is_kept_and_a_listed_empty_one_refuses() {
        let session = extended(&meerkat_core::Session::new(), &["opening"]);
        let id = session.id().to_string();
        let empty: Arc<dyn SessionStore> = Arc::new(BodiesOnly::default());
        let runtime = runtime_with(Some(&session), Catalog::Entry(None), &id);

        let (class, detail) = classify(&id, &runtime, false, &empty).await;
        assert_eq!(class, SessionCensusClass::ProjectionMissing, "{detail}");
        assert!(class.admitted());

        let (class, detail) = classify(&id, &runtime, true, &empty).await;
        assert_eq!(class, SessionCensusClass::Unreadable, "{detail}");
        assert!(detail.contains("holds no body"), "{detail}");
    }

    #[test]
    fn only_session_runtime_ids_name_a_session() {
        let id = SessionId::new();
        assert_eq!(
            session_of_runtime(&LogicalRuntimeId::for_session(&id).0),
            Some(id.clone())
        );
        assert_eq!(
            session_of_runtime(&LogicalRuntimeId::legacy_session_uuid_alias(&id).0),
            Some(id.clone())
        );
        for foreign in [
            "rt:mob:team".to_owned(),
            "rt:session:not-a-uuid".to_owned(),
            format!("rt:member:{id}"),
            format!("session:{id}"),
            String::new(),
        ] {
            assert_eq!(session_of_runtime(&foreign), None, "{foreign}");
        }
    }

    /// A session compacted twice whose graph retention re-anchored it past
    /// its oldest rewrite: Core keeps only the newest occurrence's body.
    fn reanchored() -> meerkat_core::Session {
        let mut session = twice_compacted();
        let retired = session
            .retire_transcript_history(meerkat_core::TranscriptHistoryRetention::new(
                std::num::NonZeroUsize::new(1).unwrap(),
            ))
            .unwrap();
        assert_eq!(retired, 1);
        session
    }

    fn twice_compacted() -> meerkat_core::Session {
        let mut session = extended(
            &meerkat_core::Session::new(),
            &["opening", "second", "third"],
        );
        commit_test_compaction(&mut session, 3, "first summary");
        session.push(message("fourth"));
        commit_test_compaction(&mut session, 2, "second summary");
        session
    }

    async fn bodies_of(session: &meerkat_core::Session) -> Arc<dyn SessionStore> {
        let bodies = BodiesOnly::default();
        bodies.save(session).await.unwrap();
        Arc::new(bodies)
    }

    /// The census asks Core's own head-canonical layout of the session the
    /// apply would write. A graph re-anchored past its oldest rewrite lays
    /// out from its retired base (Core's retained layout, the one the
    /// conversion seeds and an import crosses), so it is admitted with the
    /// class its copies prove, as is the same history before re-anchoring.
    /// Control: a live tail that does not extend the committed head has no
    /// layout, so the census refuses it.
    #[tokio::test]
    async fn the_census_admits_only_what_core_can_lay_out() {
        let session = reanchored();
        let id = session.id().to_string();
        let projection = bodies_of(&session).await;
        for (runtime, expected) in [
            (
                runtime_with(Some(&session), Catalog::Entry(None), &id),
                SessionCensusClass::Equal,
            ),
            (
                runtime_with(None, Catalog::Absent, &id),
                SessionCensusClass::RuntimeMissing,
            ),
        ] {
            let (class, detail) = classify(&id, &runtime, true, &projection).await;
            assert_eq!(class, expected, "{detail}");
        }

        let mut diverged = twice_compacted();
        diverged.replace_messages_unaudited_for_test(vec![message("never committed")]);
        let (class, detail) =
            layout_checked(SessionCensusClass::Equal, "control".to_owned(), &diverged);
        assert_eq!(class, SessionCensusClass::LayoutRefused, "{detail}");
        assert!(!SessionCensusClass::LayoutRefused.admitted());

        let whole = twice_compacted();
        let id = whole.id().to_string();
        let projection = bodies_of(&whole).await;
        let (class, detail) = classify(
            &id,
            &runtime_with(Some(&whole), Catalog::Entry(None), &id),
            true,
            &projection,
        )
        .await;
        assert_eq!(class, SessionCensusClass::Equal, "{detail}");
    }

    fn video_message(text: &str) -> meerkat_core::Message {
        meerkat_core::Message::User(meerkat_core::types::UserMessage::with_blocks(vec![
            meerkat_core::types::ContentBlock::Text {
                text: text.to_owned(),
            },
            meerkat_core::types::ContentBlock::Video {
                media_type: "video/mp4".to_owned(),
                duration_ms: 1_000,
                data: meerkat_core::types::VideoData::Inline {
                    data: "AAAAIGZ0eXBpc29t".to_owned(),
                },
            },
        ]))
    }

    /// A history compacted after a turn that carried an inline video, as
    /// 0.9.3 wrote such a body: the video sits inline in the compaction's
    /// delta rows, since video has no blob form. Core lays the session out
    /// with the video as row content, so the census admits it with the class
    /// its copies prove.
    #[test]
    fn a_compaction_over_an_inline_video_is_admitted() {
        let mut session = extended(&meerkat_core::Session::new(), &["opening", "second"]);
        commit_test_compaction(&mut session, 2, "first summary");
        session.push(video_message("watch this"));
        session.push(message("after the video"));
        commit_test_compaction(&mut session, 2, "second summary");
        let (class, detail) = layout_checked(
            SessionCensusClass::Equal,
            "inline video".to_owned(),
            &session,
        );
        assert_eq!(class, SessionCensusClass::Equal, "{detail}");
    }

    /// A session that compacted twice with a tool result's inline image in
    /// the second compaction's delta, as 0.9.3 wrote such a body when a
    /// session compacted after a tool returned a screenshot. Core lays it
    /// out as head-canonical rows only with the image moved to the blob
    /// store.
    pub(in crate::storage_migrate_native) fn compacted_over_a_tool_result_image(
        data: &str,
    ) -> meerkat_core::Session {
        let user =
            |text: &str| meerkat_core::Message::User(meerkat_core::types::UserMessage::text(text));
        let mut session = meerkat_core::Session::new();
        session.push(user("opening"));
        session.push(user("second"));
        commit_test_compaction(&mut session, 2, "first summary");
        session.push(meerkat_core::Message::ToolResults {
            results: vec![meerkat_core::types::ToolResult::with_blocks(
                "view-image-call".to_owned(),
                vec![
                    meerkat_core::types::ContentBlock::Text {
                        text: "the screenshot".to_owned(),
                    },
                    meerkat_core::types::ContentBlock::Image {
                        media_type: "image/png".to_owned(),
                        data: meerkat_core::types::ImageData::Inline {
                            data: data.to_owned(),
                        },
                    },
                ],
                false,
            )],
            created_at: chrono::Utc::now(),
        });
        session.push(user("after the image"));
        commit_test_compaction(&mut session, 2, "second summary");
        session
    }

    /// A projection holding `sessions` as given, rewrite histories included.
    async fn bodies_holding(sessions: &[&meerkat_core::Session]) -> Arc<dyn SessionStore> {
        let bodies = BodiesOnly::default();
        for session in sessions {
            bodies.save(session).await.unwrap();
        }
        Arc::new(bodies)
    }

    /// The `RuntimeMissing` row of `session`, as the census classified it.
    fn runtime_missing_row(session: &meerkat_core::Session) -> SessionCensusRow {
        SessionCensusRow::new(
            session.id().to_string(),
            SessionCensusClass::RuntimeMissing,
            Vec::new(),
            "the projection copy is the seed".to_owned(),
            None,
            false,
        )
    }

    /// The census runs the import itself (I1-I4) on every copy it admits as
    /// `RuntimeMissing`, against a scratch state directory of its own: a copy
    /// the import refuses is refused before `--apply` writes anything, with
    /// the import's refusal, and the census still takes every other row. Here
    /// the import refuses at I2: the scratch session store already holds a
    /// different row for the session.
    #[tokio::test]
    async fn a_copy_the_trial_import_refuses_is_refused_by_the_census() {
        let refused = extended(&meerkat_core::Session::new(), &["opening", "second"]);
        let admitted = compacted_over_a_tool_result_image("iVBORw0KGgo=");
        let projection = bodies_holding(&[&refused, &admitted]).await;
        let scratch = tempfile::tempdir().unwrap();
        {
            let trial_dir = scratch.path().join(IMPORT_TRIAL_DIR_NAME);
            std::fs::create_dir_all(&trial_dir).unwrap();
            let trial =
                meerkat_store::SqliteSessionStore::open(trial_dir.join(RUNTIME_DB_FILE_NAME))
                    .unwrap();
            let mut other = refused.clone();
            other.push(message("a turn the copy does not hold"));
            trial.save(&other).await.unwrap();
        }
        let mut rows = vec![
            runtime_missing_row(&refused),
            runtime_missing_row(&admitted),
        ];

        predict_import_rows(None, scratch.path(), &projection, &mut rows)
            .await
            .unwrap();

        assert_eq!(
            rows[0].class,
            SessionCensusClass::LayoutRefused,
            "{}",
            rows[0].detail
        );
        assert!(!rows[0].admitted);
        assert!(
            rows[0].detail.starts_with(&format!(
                "the projection copy is the seed; a trial import into a scratch directory \
                 refuses it: session {}: the session store already holds a different row (",
                refused.id()
            )),
            "{}",
            rows[0].detail
        );
        assert_eq!(
            rows[1].class,
            SessionCensusClass::RuntimeMissing,
            "{}",
            rows[1].detail
        );
        assert!(
            rows[1]
                .detail
                .ends_with("; the session store holds no row for it, so the import creates one"),
            "{}",
            rows[1].detail
        );
    }

    /// A copy the trial import takes (I1-I4: created, crossed and activated),
    /// inline images and a compacted history included, stays admitted with
    /// the detail it had, and the census's copy of `runtime.sqlite` is not
    /// written: the trial runs in a scratch directory of its own.
    #[tokio::test]
    async fn a_copy_the_trial_import_takes_stays_admitted() {
        let session = compacted_over_a_tool_result_image("iVBORw0KGgo=");
        let projection = bodies_holding(&[&session]).await;
        let scratch = tempfile::tempdir().unwrap();
        let runtime_copy = scratch.path().join(RUNTIME_DB_FILE_NAME);
        drop(meerkat_store::SqliteSessionStore::open(runtime_copy.clone()).unwrap());
        let mut rows = vec![runtime_missing_row(&session)];

        predict_import_rows(Some(&runtime_copy), scratch.path(), &projection, &mut rows)
            .await
            .unwrap();

        assert_eq!(
            rows[0].class,
            SessionCensusClass::RuntimeMissing,
            "{}",
            rows[0].detail
        );
        assert!(rows[0].admitted);
        assert_eq!(
            rows[0].detail,
            "the projection copy is the seed; the session store holds no row for it, so the \
             import creates one"
        );
        let census_copy = meerkat_store::SqliteSessionStore::open(runtime_copy).unwrap();
        assert!(census_copy.load(session.id()).await.unwrap().is_none());
    }

    /// A history compacted after a tool returned an image inline, as 0.9.3
    /// wrote such a body: the image sits inline in the tool result rows of
    /// the compaction's delta. The census moves the images to the blob store
    /// the way `--apply` does, through Core's pure transform, and admits the
    /// session with the class its copies prove.
    #[test]
    fn a_compaction_over_an_inline_tool_result_image_is_admitted() {
        let session = compacted_over_a_tool_result_image("iVBORw0KGgo=");
        let (class, detail) = layout_checked(
            SessionCensusClass::Equal,
            "the copies agree".to_owned(),
            &session,
        );
        assert_eq!(class, SessionCensusClass::Equal, "{detail}");
        assert!(
            detail.contains("1 inline image moves to the blob store"),
            "{detail}"
        );
    }

    /// A history compacted seven times and re-anchored, whose every
    /// compaction swallowed a tool result with two inline images and an
    /// inbound comms notice with one, as a 0.8.x-era host wrote them: the
    /// census admits it, with the images its retained history holds moved
    /// to the blob store.
    #[test]
    fn a_re_anchored_history_with_tool_result_and_notice_images_is_admitted() {
        let image = |data: &str| meerkat_core::types::ContentBlock::Image {
            media_type: "image/png".to_owned(),
            data: meerkat_core::types::ImageData::Inline {
                data: data.to_owned(),
            },
        };
        use base64::Engine as _;
        let mut session = extended(&meerkat_core::Session::new(), &["start"]);
        for round in 0..7 {
            let data = |slot: char| {
                base64::engine::general_purpose::STANDARD.encode(format!("PNG r{round}{slot}"))
            };
            session.push(meerkat_core::Message::tool_results(vec![
                meerkat_core::types::ToolResult::with_blocks(
                    format!("camera-{round}"),
                    vec![image(&data('a')), image(&data('b'))],
                    false,
                ),
            ]));
            session.push(meerkat_core::Message::SystemNotice(
                meerkat_core::types::SystemNoticeMessage::with_block(
                    meerkat_core::types::SystemNoticeKind::Comms,
                    None,
                    meerkat_core::types::SystemNoticeBlock::Comms {
                        kind: meerkat_core::types::CommsNoticeKind::Message,
                        direction: meerkat_core::types::SystemNoticeDirection::Incoming,
                        peer: None,
                        sender_taint: None,
                        request_id: None,
                        intent: None,
                        status: None,
                        summary: None,
                        payload: None,
                        content: vec![image(&data('c'))],
                        delivery_id: None,
                        in_reply_to: None,
                    },
                ),
            ));
            let end = session.messages().len() - 1;
            commit_test_compaction(&mut session, end, "summary so far");
        }
        session.push(message("after the last compaction"));
        assert!(
            session
                .retire_transcript_history(meerkat_core::TranscriptHistoryRetention::default())
                .unwrap()
                >= 1,
            "the history re-anchors"
        );
        let (class, detail) = layout_checked(
            SessionCensusClass::Equal,
            "the copies agree".to_owned(),
            &session,
        );
        assert_eq!(class, SessionCensusClass::Equal, "{detail}");
        // The retained occurrences keep at least the last round's images.
        let moved = detail
            .split("; ")
            .find_map(|part| part.strip_suffix(" inline images move to the blob store"))
            .and_then(|count| count.parse::<usize>().ok())
            .unwrap_or_else(|| panic!("{detail}"));
        assert!(moved >= 3, "{detail}");
    }

    /// A session only the projection holds, re-anchored past its oldest
    /// rewrite: Core's store answers its retained records alone. 0.9.3's
    /// store refused to lay that graph out and 0.9.3 seeded the session
    /// slim, so the census seeds it slim too and admits it; Core's retained
    /// layout lays the seed out.
    #[tokio::test]
    async fn a_re_anchored_projection_only_session_seeds_slim() {
        let session = reanchored();
        let id = session.id().to_string();
        let dir = tempfile::tempdir().unwrap();
        let store =
            meerkat_store::SqliteSessionStore::open(dir.path().join("sessions.sqlite3")).unwrap();
        // A plain first save may not seed rewrite history: the projection's
        // blob enters through the offline create path a migration uses.
        store
            .save_authoritative_projection_if_current_revision(&session, None)
            .await
            .unwrap();
        let projection: Arc<dyn SessionStore> = Arc::new(store);
        let (class, detail) = classify(
            &id,
            &runtime_with(None, Catalog::Absent, &id),
            true,
            &projection,
        )
        .await;
        assert_eq!(class, SessionCensusClass::RuntimeMissing, "{detail}");
        assert!(detail.contains("retained rewrite records"), "{detail}");
    }

    /// One committed snapshot that does not load refuses its own row and
    /// leaves the others to be classified.
    #[tokio::test]
    async fn a_committed_snapshot_that_does_not_load_refuses_only_its_row() {
        let session = extended(&meerkat_core::Session::new(), &["opening"]);
        let id = session.id().to_string();
        let projection = projection_of(&[&session]).await;
        let mut runtime = runtime_with(None, Catalog::Entry(None), &id);
        runtime.unreadable.insert(
            id.clone(),
            format!("committed session {id}: body digest mismatch"),
        );
        assert!(runtime.known_session_ids().any(|known| known == id));
        let (class, detail) = classify(&id, &runtime, true, &projection).await;
        assert_eq!(class, SessionCensusClass::Unreadable);
        assert!(detail.contains("body digest mismatch"), "{detail}");
    }

    /// The class remedy of a suffix is the one for a session no 0.9.3 start
    /// activates, so it names no 0.9.3 operation.
    #[test]
    fn the_suffix_remedy_names_only_verified_steps() {
        let action = SessionCensusClass::ProjectionSuffix.action().unwrap();
        assert!(
            action.ends_with("keep the directory on 0.9.3 and report this census row"),
            "{action}"
        );
        for operation in ["resume", "wake", "delete", "start", "rerun"] {
            assert!(!action.contains(operation), "{operation}: {action}");
        }
    }

    /// A suffix row names the verified 0.9.3 restart only for a session one
    /// continuity record binds and 0.9.3 left live. A retired, stopped or
    /// destroyed session, one with no recorded state, one no record or two
    /// records bind, and every other class keep the class's remedy.
    #[test]
    fn the_suffix_remedy_names_the_restart_only_for_a_live_bound_session() {
        use SessionCensusClass::{
            Divergent, Equal, ProjectionAhead, ProjectionSuffix, ProjectionSuffixUnbound,
        };
        let one = ["agent:hl-14".to_owned()];
        let restart = "keep agent:hl-14 on the roster, start the directory once with 0.9.3 and \
                       stop it cleanly; 0.9.3 re-seeds the session (verified with the published \
                       binary for an idle session); then rerun the census";
        for state in [
            RuntimeState::Initializing,
            RuntimeState::Idle,
            RuntimeState::Attached,
            RuntimeState::Running,
        ] {
            let action = ProjectionSuffix
                .row_action(&one, Some(state), true)
                .unwrap();
            assert!(action.ends_with(restart), "{state:?}: {action}");
            assert_eq!(
                ProjectionSuffix.row_action(&one, Some(state), false),
                ProjectionSuffix.action().map(str::to_owned),
                "a builder layout keeps the class remedy: {state:?}"
            );
            for operation in ["resume", "wake", "delete"] {
                assert!(!action.contains(operation), "{operation}: {action}");
            }
        }
        let class_remedy = ProjectionSuffix.action().map(str::to_owned);
        for state in [
            None,
            Some(RuntimeState::Retired),
            Some(RuntimeState::Stopped),
            Some(RuntimeState::Destroyed),
        ] {
            assert_eq!(
                ProjectionSuffix.row_action(&one, state, true),
                class_remedy,
                "{state:?}"
            );
        }
        let two = ["agent:hl-14".to_owned(), "agent:hl-15".to_owned()];
        assert_eq!(
            ProjectionSuffix.row_action(&two, Some(RuntimeState::Idle), true),
            class_remedy
        );
        assert_eq!(
            ProjectionSuffix.row_action(&[], Some(RuntimeState::Idle), true),
            class_remedy
        );
        for class in [Divergent, ProjectionAhead, ProjectionSuffixUnbound, Equal] {
            assert_eq!(
                class.row_action(&one, Some(RuntimeState::Idle), true),
                class.action().map(str::to_owned),
                "{class:?}"
            );
        }
    }

    /// The relation provers compare runtime session authority, so the
    /// census is their only caller, by design.
    #[test]
    fn only_the_census_reaches_the_relation_provers() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let allowed = [
            "storage_migrate_native/census.rs",
            "storage_migrate_native/relation.rs",
        ];
        let mut stack = vec![src.clone()];
        let mut reached = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                let relative = path
                    .strip_prefix(&src)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if allowed.contains(&relative.as_str()) {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                if reaches_relation(&relative, &text) {
                    reached.push(relative);
                }
            }
        }
        assert!(
            reached.is_empty(),
            "the relation provers are reached from {reached:?}"
        );
    }

    /// The guard reads the ways its table lists: a direct path (`super::`,
    /// `self::`, `storage_migrate_native::`), a `use` group whose top level
    /// names the module, a path through the module or a `use` that renames
    /// it after a glob import of its parent, and, in the parent module, any
    /// mention of the child. It is textual: a rename through an alias of the
    /// parent (`use super::{self as p}; use p::relation as q;`) escapes it.
    #[test]
    fn the_relation_guard_reads_grouped_and_parent_paths() {
        let other = "storage_migrate_native/layout_status.rs";
        let parent = "storage_migrate_native/mod.rs";
        for (relative, text, reaches) in [
            (other, "use super::relation::durable_behind;", true),
            (other, "use crate::storage_migrate_native::relation;", true),
            (
                other,
                "use super::{layout_status::X, relation::Relation};",
                true,
            ),
            (other, "use super::{a::{b, c}, relation::X};", true),
            (other, "use super::{\n    census,\n    relation,\n};", true),
            (
                other,
                "use crate::storage_migrate_native::{census, relation};",
                true,
            ),
            (other, "use super::relation_helpers::X;", false),
            (other, "use super::{census::{relation}};", false),
            (other, "use super::census::Census;", false),
            (
                parent,
                "pub(crate) mod relation;\npub use census::X;",
                false,
            ),
            (
                parent,
                "//! `relation` holds the provers.\nmod relation;",
                false,
            ),
            (parent, "mod relation;\nfn f() { relation::prove(); }", true),
            (parent, "mod relation;\nuse relation as probe;", true),
            (
                other,
                "use super::*;\nfn f() { relation::durable_head(); }",
                true,
            ),
            (
                "unified_runtime/mod.rs",
                "use crate::storage_migrate_native::*;\nfn f() { relation::prove(); }",
                true,
            ),
            (
                other,
                "use super::*;\nfn f() { durable_relation::x(); }",
                false,
            ),
            (
                other,
                "use super::*;\n// relation::x is the census's",
                false,
            ),
            (
                other,
                "use super::*;\nuse self::relation as probe;\nfn f() { probe::prove(); }",
                true,
            ),
            (other, "use super::*;\nuse relation as probe;", true),
            (
                other,
                "use super::*;\npub(super) use {relation as probe};",
                true,
            ),
            (other, "use super::*;\nuse relationship::Probe;", false),
        ] {
            assert_eq!(
                reaches_relation(relative, text),
                reaches,
                "{relative}: {text:?}"
            );
        }
    }

    /// Whether `text`, the source of `relative` (a path under src/), names
    /// the relation module.
    fn reaches_relation(relative: &str, text: &str) -> bool {
        let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let names_relation = |rest: &str| {
            rest.strip_prefix("relation")
                .is_some_and(|after| !after.starts_with(is_ident))
        };
        for prefix in ["super::", "self::", "storage_migrate_native::"] {
            for (at, _) in text.match_indices(prefix) {
                let rest = text[at + prefix.len()..].trim_start();
                if names_relation(rest) {
                    return true;
                }
                if let Some(group) = rest.strip_prefix('{')
                    && use_group_elements(group)
                        .iter()
                        .any(|element| names_relation(element.trim_start()))
                {
                    return true;
                }
            }
        }
        // A path through the module with no prefix: after a glob import of
        // its parent (`use super::*;`, `use crate::storage_migrate_native::*;`)
        // or in the parent itself.
        if text.lines().any(|line| {
            let line = line.trim();
            !line.starts_with("//")
                && line
                    .match_indices("relation::")
                    .any(|(at, _)| !line[..at].ends_with(is_ident))
        }) {
            return true;
        }
        // A use path that starts at the module's name: after a glob import of
        // its parent, `use relation as probe;` or `use {relation as probe};`
        // renames it, and the callers then name only the alias.
        if text.lines().any(|line| {
            let line = line.trim();
            !line.starts_with("//")
                && use_path(line).is_some_and(|path| {
                    names_relation(path)
                        || path.strip_prefix('{').is_some_and(|group| {
                            use_group_elements(group)
                                .iter()
                                .any(|element| names_relation(element.trim_start()))
                        })
                })
        }) {
            return true;
        }
        // The parent declares the child and may reach it without a prefix.
        relative == "storage_migrate_native/mod.rs"
            && text.lines().any(|line| {
                let line = line.trim();
                !line.starts_with("//")
                    && !matches!(
                        line,
                        "mod relation;" | "pub(crate) mod relation;" | "pub mod relation;"
                    )
                    && line.match_indices("relation").any(|(at, word)| {
                        !line[..at].ends_with(is_ident)
                            && !line[at + word.len()..].starts_with(is_ident)
                    })
            })
    }

    /// The path of the `use` declaration `line` opens, after any visibility.
    fn use_path(line: &str) -> Option<&str> {
        let line = match line.strip_prefix("pub") {
            Some(rest) if rest.starts_with('(') => {
                rest.split_once(')').map_or(rest, |(_, after)| after)
            }
            Some(rest) => rest,
            None => line,
        };
        line.trim_start().strip_prefix("use ").map(str::trim_start)
    }

    /// The elements at the top level of a `use` group, up to the brace that
    /// closes it; `group` starts just after the opening brace.
    fn use_group_elements(group: &str) -> Vec<&str> {
        let mut depth = 0usize;
        let mut start = 0;
        let mut elements = Vec::new();
        for (at, c) in group.char_indices() {
            match c {
                '{' => depth += 1,
                '}' if depth == 0 => {
                    elements.push(&group[start..at]);
                    break;
                }
                '}' => depth -= 1,
                ',' if depth == 0 => {
                    elements.push(&group[start..at]);
                    start = at + 1;
                }
                _ => {}
            }
        }
        elements
    }

    #[test]
    fn the_report_names_the_mob_storage_it_read() {
        assert_eq!(CensusMobStore::InMemory.to_string(), "in memory");
        let path = PathBuf::from("/state/mob.sqlite3");
        assert_eq!(
            CensusMobStore::Durable { path: path.clone() }.to_string(),
            path.display().to_string()
        );
    }

    #[test]
    fn every_class_is_either_admitted_or_names_an_action() {
        for class in [
            SessionCensusClass::Equal,
            SessionCensusClass::ProjectionBehind,
            SessionCensusClass::ProjectionMissing,
            SessionCensusClass::RuntimeMissing,
            SessionCensusClass::RuntimeLifecycleOnly,
            SessionCensusClass::ProjectionSuffix,
            SessionCensusClass::ProjectionSuffixTerminal,
            SessionCensusClass::ProjectionAhead,
            SessionCensusClass::Divergent,
            SessionCensusClass::Unreadable,
            SessionCensusClass::LayoutRefused,
            SessionCensusClass::ProjectionSuffixUnbound,
        ] {
            assert_eq!(class.action().is_some(), !class.admitted(), "{class:?}");
            if class.apply_effect().is_some() {
                assert!(class.admitted(), "{class:?}");
            }
        }
    }
}

//! Durable storage for agent-created child mobs.
//!
//! Agent `mob_create` and delegate's implicit mob build child mobs through the
//! agent mob-tool state (`meerkat_mob_mcp::MobMcpState`). Without a persistent
//! root that state keeps every child in memory, so a restart loses the child
//! definition and roster even when the member sessions themselves are durable.
//!
//! [`ChildMobStorage::Durable`] gives that state Meerkat's persistent mob root
//! under the Meta state directory. Meta applies it at the bootstrap and
//! reinstall boundary, before the state is shared, in this order:
//!
//! 1. claim the child root exclusively (one owner per root, across processes
//!    and within one process; once a child may have run, held until a
//!    stand-down proves every child and Meerkat's state-owned work settled,
//!    otherwise until process exit);
//! 2. install the canonical council store, so Meerkat never creates a second
//!    council location inside the child custody database;
//! 3. install the root storage's participant custody, which must itself be
//!    durable;
//! 4. set the native persistent root, which upgrades the remaining default
//!    stores (realm profiles, and participant custody when the root storage
//!    carries none) to durable SQLite in the same directory;
//! 5. apply the child policy and the definition's realm-profile overlay on
//!    top of the durable profile store.
//!
//! The directory is Meerkat's own (`<state>/mobs`). Meta derives it from
//! Meerkat's public custody path instead of spelling it again, and the storage
//! maintenance verbs enumerate its databases through
//! [`child_mob_databases`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs2::FileExt;
use meerkat_mob::store::TemporaryCouncilStore;

use crate::storage_layout::MetaStorageLayout;

/// Lock file inside the child root. It is not a `*.db` file, so Meerkat's
/// restore scan and the maintenance enumeration both skip it.
pub(crate) const CHILD_MOB_ROOT_LOCK_FILE: &str = "child-mobs.lock";

/// Where agent-created child mobs (agent `mob_create`, delegate's implicit
/// mob) keep their event logs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChildMobStorage {
    /// Child mobs live and end with the process.
    #[default]
    ProcessBound,
    /// Child mobs persist under the layout's child root and are restored when
    /// a runtime is composed over the same layout again.
    Durable(MetaStorageLayout),
}

/// Why durable child-mob storage could not be composed or restored.
///
/// Storage faults stay distinct from permission refusals and from delivery
/// composition errors, and every native cause is preserved.
#[derive(Debug)]
#[non_exhaustive]
pub enum ChildMobStorageError {
    /// The layout declares its state directory per-process scratch.
    DeclaredEphemeralLayout { state_dir: PathBuf },
    /// No child-mob storage was selected and the root mob storage is
    /// declared persistent, so child mobs default to durable, but no Meta
    /// storage layout was recorded to put them under.
    LayoutNotRecorded,
    /// The root mob storage is not declared persistent, so the participant
    /// custody it carries would not survive the restart that durable child
    /// mobs promise to recover.
    ProcessBoundParticipantCustody,
    /// The child root could not be created or opened.
    RootUnavailable { path: PathBuf, detail: String },
    /// Another owner holds the child root.
    AlreadyClaimed { path: PathBuf, detail: String },
    /// The canonical council store did not exist and could not be created.
    CouncilStoreUnavailable { path: PathBuf },
    /// The canonical council store exists and could not be read.
    CouncilStore(crate::council_wiring::CouncilStoreError),
    /// The legacy empty realm profile store in the child root (see
    /// [`recognize_legacy_empty_realm_profiles`]) could not be inspected or
    /// converted.
    LegacyRealmProfiles { path: PathBuf, detail: String },
    /// Meerkat refused the persistent root (for example a custody database
    /// that fails ledger validation).
    Native(meerkat_mob::MobError),
    /// Meerkat's restore scan of the child root failed as a whole (for
    /// example a conflicting duplicate child database). A child whose owner
    /// could not be confirmed is not this error: it stays managed and
    /// visibly unactivated while its healthy siblings continue.
    Restore(meerkat_mob::MobError),
    /// Durable child mobs were selected for a root mob kept in a host store
    /// realm. Durable child mobs keep their event logs and custody under a
    /// local directory, which is not authority on such a realm, so there
    /// they stay process-bound (the default).
    HostStoreRealm,
}

impl std::fmt::Display for ChildMobStorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeclaredEphemeralLayout { state_dir } => write!(
                f,
                "durable child mobs need a durable state directory, but {} is declared \
                 ephemeral",
                state_dir.display()
            ),
            Self::LayoutNotRecorded => f.write_str(
                "child mobs default to durable over a persistent root mob storage, but no Meta \
                 storage layout was recorded; record the launch layout with \
                 with_meta_storage_layout or select a child mob storage explicitly",
            ),
            Self::ProcessBoundParticipantCustody => f.write_str(
                "durable child mobs need durable participant custody, but the root mob storage \
                 is not declared persistent; declare its provenance or keep child mobs \
                 process-bound",
            ),
            Self::RootUnavailable { path, detail } => write!(
                f,
                "child mob root {} could not be opened: {detail}",
                path.display()
            ),
            Self::AlreadyClaimed { path, detail } => write!(
                f,
                "child mob root {} is already owned by another runtime: {detail}",
                path.display()
            ),
            Self::CouncilStoreUnavailable { path } => write!(
                f,
                "durable child mobs need the canonical council store at {}, which could not be \
                 created",
                path.display()
            ),
            Self::CouncilStore(error) => error.fmt(f),
            Self::LegacyRealmProfiles { path, detail } => write!(
                f,
                "the legacy realm profile store {} could not be adopted: {detail}",
                path.display()
            ),
            Self::Native(error) => write!(f, "child mob storage setup failed: {error}"),
            Self::Restore(error) => write!(f, "child mob restore failed: {error}"),
            Self::HostStoreRealm => f.write_str(
                "durable child mobs are not available on a host store realm: they keep their \
                 event logs under a local directory, which such a realm does not treat as \
                 authority; agent-created child mobs (mob_create, delegate's implicit mob) stay \
                 process-bound there, which is the default",
            ),
        }
    }
}

impl std::error::Error for ChildMobStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CouncilStore(error) => Some(error),
            Self::Native(error) | Self::Restore(error) => Some(error),
            _ => None,
        }
    }
}

/// The child root Meerkat derives for a runtime root: the directory that holds
/// its realm-scoped custody database.
pub(crate) fn child_mob_root(state_dir: &Path) -> PathBuf {
    let custody = meerkat_mob_mcp::MobMcpState::persistent_forked_participant_store_path(state_dir);
    // The custody database is a direct child of the root, so the parent is
    // always present; the fallback only keeps this function total.
    custody
        .parent()
        .map_or_else(|| state_dir.to_path_buf(), Path::to_path_buf)
}

/// Meerkat's reserved realm-profile database name inside the child root.
/// Meerkat does not export it; `realm_profile_database_name_matches_native`
/// pins this spelling against the file Meerkat actually creates.
pub(crate) const REALM_PROFILE_DATABASE: &str = "realm_profiles.db";

/// The predecessor's complete realm profile table definition, as SQLite
/// keeps it in `sqlite_master.sql`, compared after collapsing whitespace
/// only. Its primary key is the implicit autoindex, which has no SQL text.
const LEGACY_REALM_PROFILE_TABLE_SQL: &str = "CREATE TABLE realm_profiles ( \
    name TEXT PRIMARY KEY, profile_json BLOB NOT NULL, \
    revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL, \
    updated_at TEXT NOT NULL )";

/// `sql` with every run of whitespace collapsed to one space, trimmed.
fn collapse_whitespace(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether the child root's realm profile database is the predecessor's
/// empty, unversioned store, and if so make it a database Meerkat
/// initializes as new.
///
/// A predecessor (MobKit 0.8.47) left `realm_profiles.db` holding exactly the
/// table `realm_profiles` (with the complete definition in
/// [`LEGACY_REALM_PROFILE_TABLE_SQL`]) and its primary-key index, with no
/// schema ledger.
/// Meerkat's mob schema domain refuses any file that owns objects but has no
/// ledger row, so the durable child composition could not open it. Only
/// when the file has exactly that shape AND no rows is the empty table
/// dropped, in one immediate transaction under Meerkat's own maintenance
/// fence for the file, while the caller holds the child-root claim; the
/// file is then empty and Meerkat's opener initializes and stamps the
/// current schema. No data exists to lose. Any other content (a row, another
/// object, a ledger, any other definition) is left untouched, and Meerkat
/// refuses or opens it exactly as before. Returns whether it converted.
///
/// The drop commits before Meerkat initializes the store, in a separate
/// step: if that initialization then fails, the file stays empty rather
/// than returning to its original bytes (a later open initializes it).
pub(crate) fn recognize_legacy_empty_realm_profiles(
    root: &Path,
) -> Result<bool, ChildMobStorageError> {
    let path = root.join(REALM_PROFILE_DATABASE);
    if !path.is_file() {
        return Ok(false);
    }
    let failed = |detail: String| ChildMobStorageError::LegacyRealmProfiles {
        path: path.clone(),
        detail,
    };
    let _fence =
        meerkat_sqlite::OperationGuard::for_database(&path).map_err(|e| failed(e.to_string()))?;
    let mut conn = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| failed(e.to_string()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| failed(e.to_string()))?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| failed(e.to_string()))?;
    if !is_legacy_empty_realm_profiles(&tx).map_err(|e| failed(e.to_string()))? {
        return Ok(false);
    }
    tx.execute_batch("DROP TABLE realm_profiles")
        .map_err(|e| failed(e.to_string()))?;
    tx.commit().map_err(|e| failed(e.to_string()))?;
    tracing::info!(
        path = %path.display(),
        "adopted the predecessor's empty, unversioned realm profile store; Meerkat \
         initializes it as new"
    );
    Ok(true)
}

/// The exact predecessor shape: the table and its autoindex and nothing
/// else, the exact columns, and no rows.
fn is_legacy_empty_realm_profiles(conn: &rusqlite::Connection) -> rusqlite::Result<bool> {
    let mut objects: Vec<(String, String, String, Option<String>)> = conn
        .prepare("SELECT type, name, tbl_name, sql FROM sqlite_master")?
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    objects.sort();
    // Exactly the table, with its complete known definition, and its
    // implicit primary-key autoindex: no other object, so no ledger.
    let exact = match objects.as_slice() {
        [
            (index_kind, index_name, index_table, index_sql),
            (table_kind, table_name, table_table, Some(table_sql)),
        ] => {
            index_kind == "index"
                && index_name == "sqlite_autoindex_realm_profiles_1"
                && index_table == "realm_profiles"
                && index_sql.is_none()
                && table_kind == "table"
                && table_name == "realm_profiles"
                && table_table == "realm_profiles"
                && collapse_whitespace(table_sql)
                    == collapse_whitespace(LEGACY_REALM_PROFILE_TABLE_SQL)
        }
        _ => false,
    };
    if !exact {
        return Ok(false);
    }
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM realm_profiles", [], |row| row.get(0))?;
    Ok(rows == 0)
}

/// Which Meerkat store owns a child-root database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildMobDatabase {
    RealmProfiles,
    ParticipantCustody,
    MobEventLog,
}

impl ChildMobDatabase {
    pub(crate) fn classify(database: &Path) -> Self {
        let name = database.file_name();
        let custody =
            meerkat_mob_mcp::MobMcpState::persistent_forked_participant_store_path(Path::new(""));
        if name == custody.file_name() {
            Self::ParticipantCustody
        } else if name.and_then(|name| name.to_str()) == Some(REALM_PROFILE_DATABASE) {
            Self::RealmProfiles
        } else {
            Self::MobEventLog
        }
    }
}

/// Every child-mob database materialized under `state_dir`: the per-mob event
/// logs and the realm-scoped profile and custody databases. Sorted.
pub(crate) fn child_mob_databases(state_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(child_mob_root(state_dir)) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("db")
        })
        .collect();
    files.sort();
    files
}

/// Exclusive ownership of one child root.
///
/// The OS lock is held for as long as this value lives. Once a restore may
/// have started a child, Meta keeps it until a proven stand-down releases it
/// (see [`retain_until_process_exit`] and [`release_retained`]).
#[derive(Debug)]
pub(crate) struct ChildMobRootClaim {
    root: PathBuf,
    _lock: std::fs::File,
}

impl ChildMobRootClaim {
    pub(crate) fn acquire(root: &Path) -> Result<Self, ChildMobStorageError> {
        let unavailable = |error: std::io::Error| ChildMobStorageError::RootUnavailable {
            path: root.to_path_buf(),
            detail: error.to_string(),
        };
        std::fs::create_dir_all(root).map_err(unavailable)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(CHILD_MOB_ROOT_LOCK_FILE))
            .map_err(unavailable)?;
        // flock is per open file description, so a second opener in this
        // process is refused exactly like one in another process.
        if let Err(error) = lock.try_lock_exclusive() {
            return Err(
                if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
                    ChildMobStorageError::AlreadyClaimed {
                        path: root.to_path_buf(),
                        detail: error.to_string(),
                    }
                } else {
                    unavailable(error)
                },
            );
        }
        Ok(Self {
            root: root.to_path_buf(),
            _lock: lock,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

/// Claims whose child mobs may have run. A claim enters here before the
/// native restore can start a child actor and stays until a stand-down
/// proves every child actor and all of Meerkat's state-owned work settled
/// ([`release_retained`]); otherwise process exit releases the OS lock.
fn retained_claims() -> &'static std::sync::Mutex<Vec<Arc<ChildMobRootClaim>>> {
    static RETAINED: std::sync::OnceLock<std::sync::Mutex<Vec<Arc<ChildMobRootClaim>>>> =
        std::sync::OnceLock::new();
    RETAINED.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Keep `claim` owned for the rest of the process.
pub(crate) fn retain_until_process_exit(claim: &Arc<ChildMobRootClaim>) {
    let mut retained = retained_claims()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !retained.iter().any(|held| Arc::ptr_eq(held, claim)) {
        retained.push(Arc::clone(claim));
    }
}

/// Stop retaining `claim` after a proven stand-down. The OS lock is
/// released when its last holder drops it; see
/// [`ChildMobRootOwnership::release`], which only reports a release once it
/// dropped that last holder itself.
pub(crate) fn release_retained(claim: &Arc<ChildMobRootClaim>) {
    retained_claims()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|held| !Arc::ptr_eq(held, claim));
}

/// How one child mob answered its terminal shutdown.
#[derive(Debug, Clone)]
pub(crate) enum ChildShutdown {
    /// Its shutdown report settled every member.
    Clean,
    /// Its report named unsettled members, or the request itself failed.
    Unsettled(String),
    /// Its report named only members whose runtime unregister was still
    /// running at the deadline. Each settlement follows its member's exact
    /// registration after the child's actor exited.
    UnregisterPending {
        detail: String,
        settlements: Vec<meerkat_mob::MemberUnregisterSettlement>,
    },
    /// Its actor was already gone. That is not a settlement report: the
    /// actor may have exited after reporting unsettled members.
    ChannelClosed,
}

/// One running incarnation of a child mob: the handle a stand-down shut
/// down. A restarted child is a different incarnation, so a clean receipt
/// never vouches for it.
#[derive(Clone)]
pub(crate) struct ChildIncarnation {
    /// The address of the incarnation's shared definition, unique while
    /// `pin` keeps it alive.
    key: usize,
    /// Keeps `key` from being reused by a later incarnation.
    pin: Option<meerkat_mob::MobHandle>,
}

impl ChildIncarnation {
    pub(crate) fn of(handle: &meerkat_mob::MobHandle) -> Self {
        Self {
            key: std::ptr::from_ref(handle.definition()) as usize,
            pin: Some(handle.clone()),
        }
    }

    #[cfg(test)]
    fn for_test(key: usize) -> Self {
        Self { key, pin: None }
    }
}

impl std::fmt::Debug for ChildIncarnation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildIncarnation")
            .field("key", &self.key)
            .field("pinned", &self.pin.is_some())
            .finish()
    }
}

/// Debt a child left at a stand-down.
///
/// Recorded decision: only a pending runtime unregister is cleared here. A
/// stuck or interrupted retirement (`RetirementStuck`,
/// `RetirementInterrupted`) and an effect whose custody is retained
/// (`EffectCustodyRetained`) carry no settlement this process can follow:
/// their owner drives them again on the next start. So they, a failed
/// unregister, a failed request, and an actor that exited before this call
/// could read its report stay [`ChildDebt::Specific`] for the rest of the
/// process, and the child root stays claimed. That fails closed by design:
/// the child root is never released while debt that nothing here can prove
/// settled may still touch it.
#[derive(Debug, Clone)]
enum ChildDebt {
    /// Never cleared here.
    Specific(String),
    /// Only members whose runtime unregister was still running at the
    /// deadline, one entry per incarnation that reported them. An entry is
    /// cleared once each of its exact registrations' unregisters settled
    /// ([`ChildMobRootOwnership::settle_pending_unregisters`]); the debt is
    /// gone with its last entry.
    PendingUnregister(Vec<IncarnationUnregisters>),
}

/// The runtime unregisters one incarnation of a child reported still
/// running at the deadline. Kept per incarnation: their settling vouches for
/// that incarnation only, never for a later one of the same child.
#[derive(Debug, Clone)]
struct IncarnationUnregisters {
    detail: String,
    incarnation: ChildIncarnation,
    settlements: Vec<meerkat_mob::MemberUnregisterSettlement>,
}

impl ChildDebt {
    /// What the child reported: for pending unregisters, each incarnation's
    /// report in the order they were recorded.
    fn detail(&self) -> String {
        match self {
            Self::Specific(detail) => detail.clone(),
            Self::PendingUnregister(entries) => entries
                .iter()
                .map(|entry| entry.detail.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        }
    }

    fn kind(&self) -> RetainedChildMobKind {
        match self {
            Self::Specific(_) => RetainedChildMobKind::Specific,
            Self::PendingUnregister(_) => RetainedChildMobKind::PendingUnregister,
        }
    }
}

/// Where a child's pending runtime unregisters stand together.
pub(crate) enum PendingUnregisters {
    /// Every one settled.
    Settled,
    /// At least one is still running, and none failed.
    Pending,
    /// At least one failed: nothing settles it later.
    Failed(String),
}

impl PendingUnregisters {
    /// Where `states`, one read from each settlement, stand together. Each
    /// state Meerkat defines is mapped by name.
    pub(crate) fn of(states: &[meerkat_mob::MemberUnregisterState]) -> Self {
        use meerkat_mob::MemberUnregisterState as State;
        let mut failures = Vec::new();
        let mut running = false;
        for state in states {
            match state {
                // `HandedOver`: a later owner took the registration over
                // after the child's Shutdown returned (a resumed child, or
                // any caller that reused the registration or attached to it
                // in place). Nothing was unregistered and nothing will be:
                // the registration is that owner's, so the incarnation that
                // reported it owes nothing more for it. It settles that
                // incarnation's debt only and vouches for no other one.
                State::Settled | State::HandedOver => {}
                State::Pending => running = true,
                State::Failed(failure) => failures.push(failure.clone()),
                // A state this build does not know: nothing here can prove
                // it settled, so it keeps the child root like a failure.
                unknown => failures.push(format!("unregister state not understood: {unknown:?}")),
            }
        }
        if !failures.is_empty() {
            Self::Failed(failures.join("; "))
        } else if running {
            Self::Pending
        } else {
            Self::Settled
        }
    }
}

#[derive(Debug, Default)]
struct OwnershipState {
    claim: Option<Arc<ChildMobRootClaim>>,
    /// How many child mobs the stand-down that released the claim shut down;
    /// set once the claim is released.
    released_child_mobs: Option<usize>,
    /// The debt seen for each child, so a repeat call reports it again
    /// instead of reading a closed channel as clean. Meerkat keeps the full
    /// report itself; this remembers which debt was seen, and follows a
    /// pending runtime unregister until it settles.
    debt_seen: BTreeMap<String, ChildDebt>,
    /// The incarnations each child shut down cleanly in, so a retry does
    /// not ask such a stopped actor again (its closed channel would read as
    /// unknown debt). One receipt per incarnation: an earlier incarnation
    /// whose pending unregisters settle later never replaces a later
    /// incarnation's receipt, and never vouches for it.
    clean: BTreeMap<String, Vec<ChildIncarnation>>,
}

impl OwnershipState {
    /// Keep `incarnation`'s clean receipt for `mob_id`.
    fn mark_clean(&mut self, mob_id: String, incarnation: ChildIncarnation) {
        let receipts = self.clean.entry(mob_id).or_default();
        if !receipts.iter().any(|clean| clean.key == incarnation.key) {
            receipts.push(incarnation);
        }
    }
}

/// The child-root claim and the debt seen by a runtime and all its clones.
///
/// The claim never leaves this ownership except into the release itself:
/// queries answer without cloning it, so the only holders besides this one
/// are the process-wide retention and the build that acquired it.
#[derive(Debug, Clone, Default)]
pub(crate) struct ChildMobRootOwnership {
    state: Arc<std::sync::Mutex<OwnershipState>>,
    /// Serializes stand-downs through every clone: a concurrent retry waits
    /// for the running one and then sees its result, instead of asking the
    /// same children again mid-shutdown.
    stand_down: Arc<tokio::sync::Mutex<()>>,
}

impl ChildMobRootOwnership {
    pub(crate) fn new(claim: Option<Arc<ChildMobRootClaim>>) -> Self {
        Self {
            state: Arc::new(std::sync::Mutex::new(OwnershipState {
                claim,
                ..OwnershipState::default()
            })),
            stand_down: Arc::default(),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, OwnershipState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Wait, until `deadline`, for any stand-down running through another
    /// clone; hold the guard for the whole stand-down. `None` when the
    /// deadline passed first: the running stand-down keeps going untouched.
    pub(crate) async fn begin_stand_down(
        &self,
        deadline: std::time::Instant,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            Arc::clone(&self.stand_down).lock_owned(),
        )
        .await
        .ok()
    }

    /// Whether the claim is still held here.
    pub(crate) fn owns_claim(&self) -> bool {
        self.state().claim.is_some()
    }

    /// The outcome to report when no claim is held here: the release that
    /// gave it up, or no child root at all. `None` while the claim is held.
    pub(crate) fn unclaimed_outcome(&self) -> Option<ChildMobStandDownOutcome> {
        let state = self.state();
        if state.claim.is_some() {
            return None;
        }
        Some(
            state
                .released_child_mobs
                .map_or(ChildMobStandDownOutcome::NotConfigured, |child_mobs| {
                    ChildMobStandDownOutcome::Released { child_mobs }
                }),
        )
    }

    /// Give up the claim after a proven stand-down, in one transition: the
    /// claim leaves the process-wide retention and this ownership, and the
    /// release is published only once this call dropped the last holder, so
    /// the OS lock is already gone when any clone can read `Released`. If
    /// another holder is still alive, the claim goes back where it was and
    /// the root stays held; a later stand-down retries.
    pub(crate) fn release(&self, child_mobs: usize) -> ChildMobStandDownOutcome {
        let mut state = self.state();
        let Some(claim) = state.claim.take() else {
            return state
                .released_child_mobs
                .map_or(ChildMobStandDownOutcome::NotConfigured, |child_mobs| {
                    ChildMobStandDownOutcome::Released { child_mobs }
                });
        };
        release_retained(&claim);
        match Arc::try_unwrap(claim) {
            Ok(last) => {
                drop(last);
                state.released_child_mobs = Some(child_mobs);
                ChildMobStandDownOutcome::Released { child_mobs }
            }
            Err(claim) => {
                retain_until_process_exit(&claim);
                state.claim = Some(claim);
                ChildMobStandDownOutcome::HeldUntilProcessExit { child_mobs }
            }
        }
    }

    /// A second holder of the claim, standing in for one that outlives the
    /// release boundary.
    #[cfg(test)]
    pub(crate) fn claim_holder_for_test(&self) -> Option<Arc<ChildMobRootClaim>> {
        self.state().claim.clone()
    }

    /// Whether `mob_id` already shut down cleanly in exactly `incarnation`.
    pub(crate) fn clean_in(&self, mob_id: &str, incarnation: &ChildIncarnation) -> bool {
        self.state()
            .clean
            .get(mob_id)
            .is_some_and(|receipts| receipts.iter().any(|clean| clean.key == incarnation.key))
    }

    /// Whether `mob_id` holds a pending runtime unregister reported by
    /// exactly `incarnation`. Its actor exited after that report, so it is
    /// not asked again: asking would only read its closed channel.
    /// [`Self::settle_pending_unregisters`] follows the debt instead.
    pub(crate) fn pending_in(&self, mob_id: &str, incarnation: &ChildIncarnation) -> bool {
        matches!(
            self.state().debt_seen.get(mob_id),
            Some(ChildDebt::PendingUnregister(entries))
                if entries.iter().any(|entry| entry.incarnation.key == incarnation.key)
        )
    }

    /// Follow every pending runtime unregister until `deadline`, each
    /// incarnation's entry on its own. An entry whose unregisters all settled,
    /// or whose registrations a later owner took over, is cleared, and its
    /// incarnation (that one only) counts as shut down cleanly; a failed one
    /// turns the child's debt specific; one still running stays pending for
    /// a later call. A child's debt is gone with its last entry.
    ///
    /// [`crate::MobRuntime::stand_down_child_mobs`] calls this only after it
    /// asked every child incarnation hosted now (admission closed and
    /// settled, the state sealed), so an earlier incarnation's settlement is
    /// never followed while a later incarnation of the same child may still
    /// hold that member's registration.
    pub(crate) async fn settle_pending_unregisters(&self, deadline: std::time::Instant) {
        let pending = {
            let state = self.state();
            state
                .debt_seen
                .iter()
                .flat_map(|(mob_id, debt)| match debt {
                    ChildDebt::PendingUnregister(entries) => entries
                        .iter()
                        .map(|entry| {
                            (
                                mob_id.clone(),
                                entry.incarnation.key,
                                entry.settlements.clone(),
                            )
                        })
                        .collect::<Vec<_>>(),
                    ChildDebt::Specific(_) => Vec::new(),
                })
                .collect::<Vec<_>>()
        };
        if pending.is_empty() {
            return;
        }
        let followed = futures::future::join_all(pending.into_iter().map(
            |(mob_id, key, mut settlements)| async move {
                let states = futures::future::join_all(
                    settlements
                        .iter_mut()
                        .map(|settlement| settlement.settle_within(deadline)),
                )
                .await;
                (mob_id, key, settlements, PendingUnregisters::of(&states))
            },
        ))
        .await;
        let mut state = self.state();
        for (mob_id, key, settlements, settled) in followed {
            // Only the entry this call followed, as it was followed: the
            // same child, the same incarnation and the same settlements.
            // Stand-downs are serialized through every clone
            // (`begin_stand_down`) and `record` runs only inside one, so
            // nothing joins an entry while it is followed; an entry that
            // changed anyway is left for a later call, and a debt that turned
            // specific meanwhile stays specific.
            let Some(ChildDebt::PendingUnregister(entries)) = state.debt_seen.get_mut(&mob_id)
            else {
                continue;
            };
            let Some(at) = entries.iter().position(|entry| {
                entry.incarnation.key == key && entry.settlements.len() == settlements.len()
            }) else {
                continue;
            };
            match settled {
                PendingUnregisters::Settled => {
                    let entry = entries.remove(at);
                    if entries.is_empty() {
                        state.debt_seen.remove(&mob_id);
                    }
                    state.mark_clean(mob_id, entry.incarnation);
                }
                PendingUnregisters::Failed(failure) => {
                    let detail = format!("{}; {failure}", entries[at].detail);
                    state.debt_seen.insert(mob_id, ChildDebt::Specific(detail));
                }
                PendingUnregisters::Pending => entries[at].settlements = settlements,
            }
        }
    }

    /// Record how `mob_id` answered a stand-down in `incarnation`. Specific
    /// debt is never cleared; a pending runtime unregister clears once its
    /// exact registration's coordinator reports it settled, or the
    /// registration handed over to a later owner
    /// ([`Self::settle_pending_unregisters`]). Like Meerkat's own report, a
    /// later clean answer does not overwrite outstanding debt, and specific
    /// debt seen later replaces a pending unregister. Each incarnation's
    /// pending unregisters stay its own entry. A clean answer is kept as that
    /// incarnation's receipt.
    pub(crate) fn record(
        &self,
        mob_id: &str,
        incarnation: ChildIncarnation,
        outcome: ChildShutdown,
    ) {
        let debt = match outcome {
            ChildShutdown::Clean => {
                self.state().mark_clean(mob_id.to_string(), incarnation);
                return;
            }
            ChildShutdown::Unsettled(detail) => ChildDebt::Specific(detail),
            ChildShutdown::ChannelClosed => ChildDebt::Specific(
                "actor exited; its settlement is unknown to this call".to_string(),
            ),
            ChildShutdown::UnregisterPending {
                detail,
                settlements,
            } => ChildDebt::PendingUnregister(vec![IncarnationUnregisters {
                detail,
                incarnation,
                settlements,
            }]),
        };
        let mut state = self.state();
        let merged = match state.debt_seen.remove(mob_id) {
            None => debt,
            // Specific debt stays, whatever is seen later.
            Some(specific @ ChildDebt::Specific(_)) => specific,
            Some(ChildDebt::PendingUnregister(mut earlier)) => match debt {
                // Specific debt seen later replaces a pending unregister.
                specific @ ChildDebt::Specific(_) => specific,
                // A later incarnation's pending unregisters join as their
                // own entry: each settles, and vouches, for its own
                // incarnation only.
                ChildDebt::PendingUnregister(later) => {
                    for entry in later {
                        match earlier
                            .iter()
                            .position(|known| known.incarnation.key == entry.incarnation.key)
                        {
                            Some(at) => earlier[at].settlements.extend(entry.settlements),
                            None => earlier.push(entry),
                        }
                    }
                    ChildDebt::PendingUnregister(earlier)
                }
            },
        };
        state.debt_seen.insert(mob_id.to_string(), merged);
    }

    /// The stand-down result over everything seen so far. The claim is never
    /// released here; see [`retain_until_process_exit`].
    pub(crate) fn outcome(&self, child_mobs: usize) -> ChildMobStandDownOutcome {
        let state = self.state();
        if state.debt_seen.is_empty() {
            ChildMobStandDownOutcome::HeldUntilProcessExit { child_mobs }
        } else {
            ChildMobStandDownOutcome::Retained {
                unsettled: state
                    .debt_seen
                    .iter()
                    .map(|(mob_id, debt)| RetainedChildMob {
                        mob_id: mob_id.clone(),
                        kind: debt.kind(),
                        detail: debt.detail(),
                    })
                    .collect(),
            }
        }
    }
}

/// One child mob's debt in [`ChildMobStandDownOutcome::Retained`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RetainedChildMob {
    /// The child mob.
    pub mob_id: String,
    /// Whether a later stand-down in this process can clear the debt.
    pub kind: RetainedChildMobKind,
    /// What the child reported, for diagnostics; not a stable format.
    pub detail: String,
}

/// Whether a later stand-down in this process can clear a child mob's debt
/// ([`RetainedChildMob::kind`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetainedChildMobKind {
    /// Only member runtime unregisters still running at the deadline. A
    /// later stand-down follows each one and releases the child root once
    /// they settled (or a later owner took their registrations over) and
    /// nothing else is owed. One that fails instead turns the debt
    /// [`Self::Specific`].
    PendingUnregister,
    /// Debt nothing in this process can prove settled: a failed or unknown
    /// unregister state, a stuck or interrupted retirement, effect custody
    /// retained, a failed shutdown request, or an actor that exited before
    /// its report could be read. It keeps the child root owned until the
    /// process exits; no later stand-down clears it.
    Specific,
}

/// What happened to durable child mobs and their root claim when a runtime
/// stood them down. The claim is released only in
/// [`ChildMobStandDownOutcome::Released`]; every other durable outcome keeps
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChildMobStandDownOutcome {
    /// Child mobs are process-bound: there is no child root.
    NotConfigured,
    /// Every child actor reported a clean shutdown, but the agent mob-tool
    /// state did not settle before the deadline (a create admitted before
    /// its close still writing, a council run, a completion custodian or a
    /// live delegation delivery still running), the runtime's delivery owner
    /// was still running at the deadline, or another holder of the root
    /// claim was still alive at the release, so cleanup is NOT complete.
    /// The child root stays owned (a later stand-down retries); a successor
    /// in this process is refused with
    /// [`ChildMobStorageError::AlreadyClaimed`].
    HeldUntilProcessExit {
        /// How many child mobs were shut down.
        child_mobs: usize,
    },
    /// Every child actor reported a clean shutdown and Meerkat's state
    /// joined all the work it owns and sealed itself, so the child root was
    /// released: a successor over the same layout in this process can claim
    /// it.
    Released {
        /// How many child mobs were shut down.
        child_mobs: usize,
    },
    /// At least one child mob left debt, now or on an earlier call. Each
    /// entry's [`RetainedChildMobKind`] says whether a later stand-down can
    /// clear it: one follows a member's runtime unregister still running at
    /// the deadline and releases the root once it settles (and nothing else
    /// is owed); specific or unknown debt keeps the child root for the rest
    /// of the process.
    Retained {
        /// One entry per child mob with debt.
        unsettled: Vec<RetainedChildMob>,
    },
    /// The root mob did not stop, so child mobs were not stood down.
    SkippedMobStopFailed,
    /// Another stand-down of this child root, through a clone of the
    /// runtime, was still running when this call's deadline passed, so this
    /// call observed nothing about the children and cleanup is NOT complete.
    /// The running stand-down is not cancelled; the child root stays owned
    /// until it (or a later call) releases it.
    InProgress,
    /// The child mobs could not be listed, or one could not be looked up,
    /// so this call could not ask every child and cleanup is NOT complete.
    /// Debt recorded by this or an earlier call stays recorded; the child
    /// root stays owned, and a later stand-down lists the children again.
    EnumerationFailed {
        /// Why the listing or the lookup failed.
        detail: String,
    },
}

impl ChildMobStandDownOutcome {
    /// Whether every observed child actor reported a clean shutdown (or
    /// there were none). Says nothing about Meerkat's state-owned work.
    #[must_use]
    pub fn child_actors_settled(&self) -> bool {
        matches!(
            self,
            Self::NotConfigured | Self::HeldUntilProcessExit { .. } | Self::Released { .. }
        )
    }

    /// Whether child-mob cleanup is complete: there is no durable child
    /// root, or it was released after a proven stand-down.
    #[must_use]
    pub fn cleanup_complete(&self) -> bool {
        matches!(self, Self::NotConfigured | Self::Released { .. })
    }
}

/// What a durable child install needs, resolved and validated before the
/// agent mob-tool state is built.
pub(crate) struct DurableChildMobs {
    /// The runtime root handed to Meerkat; it derives the child root from it.
    pub(crate) runtime_root: PathBuf,
    /// The canonical council store, required for durable child mobs.
    pub(crate) council_store: Arc<dyn TemporaryCouncilStore>,
}

/// Open the canonical council store for a durable child composition.
///
/// Unlike [`crate::council_wiring::open_council_store`], an absent file that
/// cannot be created is a failure here: falling back to an in-memory store
/// would let Meerkat create a second council location in the child custody
/// database.
pub(crate) fn open_required_council_store(
    layout: &MetaStorageLayout,
) -> Result<Arc<dyn TemporaryCouncilStore>, ChildMobStorageError> {
    let path = layout.council_db();
    match crate::council_wiring::open_council_store(&path) {
        Ok(Some(store)) => Ok(store),
        Ok(None) => Err(ChildMobStorageError::CouncilStoreUnavailable { path }),
        Err(error) => Err(ChildMobStorageError::CouncilStore(error)),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const LEGACY_EMPTY_REALM_PROFILES: &[u8] =
        include_bytes!("../tests/fixtures/legacy_empty_realm_profiles/realm_profiles.db");

    /// A child root holding the production predecessor's empty realm
    /// profile store, optionally changed by `adjust` first.
    fn legacy_root(adjust: impl FnOnce(&rusqlite::Connection)) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("child root");
        let path = dir.path().join(REALM_PROFILE_DATABASE);
        std::fs::write(&path, LEGACY_EMPTY_REALM_PROFILES).expect("plant the fixture");
        let conn = rusqlite::Connection::open(&path).expect("open the fixture");
        adjust(&conn);
        drop(conn);
        dir
    }

    fn native_open(root: &Path) -> Result<(), String> {
        meerkat_mob::SqliteRealmProfileStore::open(&root.join(REALM_PROFILE_DATABASE))
            .map(drop)
            .map_err(|error| error.to_string())
    }

    fn table_rows(root: &Path) -> Vec<(String, String)> {
        let conn = rusqlite::Connection::open(root.join(REALM_PROFILE_DATABASE)).expect("open");
        let mut rows: Vec<(String, String)> = conn
            .prepare("SELECT type, name FROM sqlite_master")
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("rows");
        rows.sort();
        rows
    }

    /// The production file: Meerkat refuses it as it is; after recognition
    /// Meerkat opens it as a new store, and a second pass is a no-op.
    #[test]
    fn the_predecessors_empty_realm_profile_store_is_adopted() {
        let root = legacy_root(|_| {});
        let refused = native_open(root.path()).expect_err("Meerkat refuses the legacy file");
        assert!(refused.contains("no ledger row"), "{refused}");
        assert!(recognize_legacy_empty_realm_profiles(root.path()).expect("recognize"));
        native_open(root.path()).expect("Meerkat initializes the adopted store");
        assert!(
            table_rows(root.path())
                .iter()
                .any(|(kind, name)| kind == "table" && name == "realm_profiles"),
            "Meerkat created its own realm profile table"
        );
        assert!(
            !recognize_legacy_empty_realm_profiles(root.path()).expect("recognize again"),
            "a store Meerkat owns is never touched"
        );
        native_open(root.path()).expect("reopens");
    }

    /// Anything but the exact empty shape is left byte-for-byte alone, and
    /// Meerkat refuses it exactly as before.
    #[test]
    fn any_other_legacy_realm_profile_store_is_left_to_meerkat() {
        type Adjust = Box<dyn FnOnce(&rusqlite::Connection)>;
        let cases: [(&str, Adjust); 4] = [
            (
                "a row",
                Box::new(|conn| {
                    conn.execute(
                        "INSERT INTO realm_profiles (name, profile_json, created_at, updated_at) \
                         VALUES ('p', x'7b7d', 'now', 'now')",
                        [],
                    )
                    .expect("insert");
                }),
            ),
            (
                "another table",
                Box::new(|conn| {
                    conn.execute_batch("CREATE TABLE extra (id INTEGER)")
                        .expect("extra table");
                }),
            ),
            (
                "another index",
                Box::new(|conn| {
                    conn.execute_batch(
                        "CREATE INDEX realm_profiles_rev ON realm_profiles (revision)",
                    )
                    .expect("extra index");
                }),
            ),
            (
                "another column",
                Box::new(|conn| {
                    conn.execute_batch("ALTER TABLE realm_profiles ADD COLUMN owner TEXT")
                        .expect("extra column");
                }),
            ),
        ];
        for (case, adjust) in cases {
            let root = legacy_root(adjust);
            let before = std::fs::read(root.path().join(REALM_PROFILE_DATABASE)).expect("read");
            assert!(
                !recognize_legacy_empty_realm_profiles(root.path()).expect("inspect"),
                "{case}: not the exact empty shape"
            );
            assert_eq!(
                std::fs::read(root.path().join(REALM_PROFILE_DATABASE)).expect("read"),
                before,
                "{case}: left byte-for-byte alone"
            );
            assert!(
                native_open(root.path()).is_err(),
                "{case}: Meerkat still refuses it"
            );
        }
    }

    /// A table with the same name and the same columns but any other
    /// definition (a collation, a CHECK, a generated column default) is not
    /// the predecessor's store: left byte-for-byte alone and refused.
    #[test]
    fn a_same_columns_different_definition_store_is_left_to_meerkat() {
        for (case, ddl) in [
            (
                "collation",
                "CREATE TABLE realm_profiles (name TEXT PRIMARY KEY COLLATE NOCASE, \
                 profile_json BLOB NOT NULL, revision INTEGER NOT NULL DEFAULT 1, \
                 created_at TEXT NOT NULL, updated_at TEXT NOT NULL)",
            ),
            (
                "check",
                "CREATE TABLE realm_profiles (name TEXT PRIMARY KEY, \
                 profile_json BLOB NOT NULL, revision INTEGER NOT NULL DEFAULT 1 \
                 CHECK (revision > 0), created_at TEXT NOT NULL, updated_at TEXT NOT NULL)",
            ),
            (
                "table constraint",
                "CREATE TABLE realm_profiles (name TEXT PRIMARY KEY, \
                 profile_json BLOB NOT NULL, revision INTEGER NOT NULL DEFAULT 1, \
                 created_at TEXT NOT NULL, updated_at TEXT NOT NULL, \
                 UNIQUE (created_at))",
            ),
            (
                "without rowid",
                "CREATE TABLE realm_profiles (name TEXT PRIMARY KEY, \
                 profile_json BLOB NOT NULL, revision INTEGER NOT NULL DEFAULT 1, \
                 created_at TEXT NOT NULL, updated_at TEXT NOT NULL) WITHOUT ROWID",
            ),
        ] {
            let dir = tempfile::tempdir().expect("child root");
            let path = dir.path().join(REALM_PROFILE_DATABASE);
            let conn = rusqlite::Connection::open(&path).expect("create");
            conn.execute_batch(ddl).expect("same-columns table");
            drop(conn);
            let before = std::fs::read(&path).expect("read");
            assert!(
                !recognize_legacy_empty_realm_profiles(dir.path()).expect("inspect"),
                "{case}: a different definition is not the predecessor's store"
            );
            assert_eq!(
                std::fs::read(&path).expect("read"),
                before,
                "{case}: left byte-for-byte alone"
            );
            assert!(
                native_open(dir.path()).is_err(),
                "{case}: Meerkat still refuses it"
            );
        }
    }

    /// No realm profile store, or one Meerkat created: nothing to do.
    #[test]
    fn a_missing_or_native_realm_profile_store_is_not_touched() {
        let dir = tempfile::tempdir().expect("child root");
        assert!(!recognize_legacy_empty_realm_profiles(dir.path()).expect("missing"));
        native_open(dir.path()).expect("Meerkat creates its store");
        assert!(!recognize_legacy_empty_realm_profiles(dir.path()).expect("native"));
        native_open(dir.path()).expect("reopens");
    }

    #[test]
    fn child_root_is_the_native_custody_directory() {
        let dir = tempfile::tempdir().expect("state dir");
        let custody =
            meerkat_mob_mcp::MobMcpState::persistent_forked_participant_store_path(dir.path());
        assert_eq!(
            Some(child_mob_root(dir.path()).as_path()),
            custody.parent(),
            "Meta must not spell the child root independently of Meerkat"
        );
        assert_eq!(
            MetaStorageLayout::with_injected_roots(dir.path().to_path_buf(), None).child_mob_root(),
            child_mob_root(dir.path())
        );
    }

    #[test]
    fn a_second_owner_is_refused_until_the_first_claim_is_dropped() {
        let dir = tempfile::tempdir().expect("state dir");
        let root = child_mob_root(dir.path());
        let first = ChildMobRootClaim::acquire(&root).expect("first owner");
        assert!(matches!(
            ChildMobRootClaim::acquire(&root),
            Err(ChildMobStorageError::AlreadyClaimed { .. })
        ));
        drop(first);
        ChildMobRootClaim::acquire(&root).expect("successor once the first claim is dropped");
    }

    #[test]
    fn unsettled_child_debt_survives_a_later_closed_channel() {
        // The actor reported an unsettled member and then exited: a later call
        // only sees a closed channel, which is not a settlement report.
        let ownership = ChildMobRootOwnership::new(None);
        let child = ChildIncarnation::for_test(1);
        ownership.record(
            "child",
            child.clone(),
            ChildShutdown::Unsettled("pending unregister".into()),
        );
        let first = ownership.outcome(1);
        ownership.record("child", child.clone(), ChildShutdown::ChannelClosed);
        ownership.record("child", child.clone(), ChildShutdown::Clean);
        assert_eq!(ownership.outcome(1), first);
        assert_eq!(
            first,
            ChildMobStandDownOutcome::Retained {
                unsettled: vec![retained(
                    "child",
                    RetainedChildMobKind::Specific,
                    "pending unregister"
                )]
            }
        );
        // A closed channel without a report this call can read is unknown
        // debt, never a clean settlement.
        let unknown = ChildMobRootOwnership::new(None);
        unknown.record("child", child.clone(), ChildShutdown::ChannelClosed);
        assert!(!unknown.outcome(1).child_actors_settled());
        // A clean report keeps the child root held and cleanup incomplete.
        let clean = ChildMobRootOwnership::new(None);
        clean.record("child", child.clone(), ChildShutdown::Clean);
        assert!(clean.clean_in("child", &child), "the clean receipt is kept");
        assert!(
            !clean.clean_in("child", &ChildIncarnation::for_test(2)),
            "a receipt never vouches for another incarnation"
        );
        let held = clean.outcome(1);
        assert_eq!(
            held,
            ChildMobStandDownOutcome::HeldUntilProcessExit { child_mobs: 1 }
        );
        assert!(held.child_actors_settled());
        assert!(!held.cleanup_complete());
    }

    #[test]
    fn specific_child_debt_outranks_a_pending_unregister() {
        let pending = || ChildShutdown::UnregisterPending {
            detail: "pending unregister".into(),
            settlements: Vec::new(),
        };
        let child = ChildIncarnation::for_test(1);
        // Specific debt seen first is never replaced.
        let specific_first = ChildMobRootOwnership::new(None);
        specific_first.record(
            "child",
            child.clone(),
            ChildShutdown::Unsettled("retirement stuck".into()),
        );
        specific_first.record("child", child.clone(), pending());
        assert!(!specific_first.pending_in("child", &child));
        assert_eq!(
            specific_first.outcome(1),
            ChildMobStandDownOutcome::Retained {
                unsettled: vec![retained(
                    "child",
                    RetainedChildMobKind::Specific,
                    "retirement stuck"
                )]
            }
        );
        // A pending unregister is followed only for the incarnation that
        // reported it, and gives way to specific debt seen later.
        let pending_first = ChildMobRootOwnership::new(None);
        pending_first.record("child", child.clone(), pending());
        assert!(pending_first.pending_in("child", &child));
        assert!(!pending_first.pending_in("child", &ChildIncarnation::for_test(2)));
        pending_first.record(
            "child",
            ChildIncarnation::for_test(2),
            ChildShutdown::ChannelClosed,
        );
        assert!(!pending_first.pending_in("child", &child));
        assert_eq!(
            pending_first.outcome(1),
            ChildMobStandDownOutcome::Retained {
                unsettled: vec![retained(
                    "child",
                    RetainedChildMobKind::Specific,
                    "actor exited; its settlement is unknown to this call"
                )]
            }
        );
    }

    fn retained(mob_id: &str, kind: RetainedChildMobKind, detail: &str) -> RetainedChildMob {
        RetainedChildMob {
            mob_id: mob_id.to_string(),
            kind,
            detail: detail.to_string(),
        }
    }

    fn pending_unregister(detail: &str) -> ChildShutdown {
        ChildShutdown::UnregisterPending {
            detail: detail.to_string(),
            settlements: Vec::new(),
        }
    }

    /// A registration a later owner took over settles the debt of the
    /// incarnation that reported it pending: nothing was unregistered and
    /// nothing will be. It never hides a failure or a still running
    /// unregister beside it.
    #[test]
    fn a_handed_over_registration_settles_its_own_pending_unregister() {
        use meerkat_mob::MemberUnregisterState as State;
        assert!(matches!(
            PendingUnregisters::of(&[State::HandedOver]),
            PendingUnregisters::Settled
        ));
        assert!(matches!(
            PendingUnregisters::of(&[State::Settled, State::HandedOver]),
            PendingUnregisters::Settled
        ));
        assert!(matches!(
            PendingUnregisters::of(&[State::HandedOver, State::Pending]),
            PendingUnregisters::Pending
        ));
        assert!(matches!(
            PendingUnregisters::of(&[State::HandedOver, State::Failed("refused".to_string())]),
            PendingUnregisters::Failed(failure) if failure == "refused"
        ));
    }

    /// The retained outcome says, per child, whether a later stand-down can
    /// clear the debt.
    #[test]
    fn a_retained_child_names_whether_a_later_stand_down_can_clear_it() {
        let ownership = ChildMobRootOwnership::new(None);
        ownership.record(
            "pending",
            ChildIncarnation::for_test(1),
            pending_unregister("pending unregister"),
        );
        ownership.record(
            "stuck",
            ChildIncarnation::for_test(2),
            ChildShutdown::Unsettled("retirement stuck".into()),
        );
        assert_eq!(
            ownership.outcome(2),
            ChildMobStandDownOutcome::Retained {
                unsettled: vec![
                    retained(
                        "pending",
                        RetainedChildMobKind::PendingUnregister,
                        "pending unregister"
                    ),
                    retained("stuck", RetainedChildMobKind::Specific, "retirement stuck"),
                ]
            }
        );
    }

    /// Each incarnation's pending unregisters are their own entry, and
    /// settling them marks that incarnation clean and no other: an earlier
    /// incarnation that settles after a later one shut down cleanly keeps
    /// the later one's receipt, and an incarnation that never answered is
    /// never vouched for.
    #[tokio::test]
    async fn a_settled_incarnation_vouches_for_itself_only() {
        let (earlier, later) = (ChildIncarnation::for_test(1), ChildIncarnation::for_test(2));

        let both_pending = ChildMobRootOwnership::new(None);
        both_pending.record("child", earlier.clone(), pending_unregister("first"));
        both_pending.record("child", later.clone(), pending_unregister("second"));
        assert!(both_pending.pending_in("child", &earlier));
        assert!(both_pending.pending_in("child", &later));
        assert_eq!(
            both_pending.outcome(1),
            ChildMobStandDownOutcome::Retained {
                unsettled: vec![retained(
                    "child",
                    RetainedChildMobKind::PendingUnregister,
                    "first; second"
                )]
            }
        );
        both_pending
            .settle_pending_unregisters(std::time::Instant::now())
            .await;
        assert!(both_pending.clean_in("child", &earlier));
        assert!(both_pending.clean_in("child", &later));
        assert!(!both_pending.clean_in("child", &ChildIncarnation::for_test(3)));
        assert_eq!(
            both_pending.outcome(1),
            ChildMobStandDownOutcome::HeldUntilProcessExit { child_mobs: 1 }
        );

        let later_clean = ChildMobRootOwnership::new(None);
        later_clean.record("child", earlier.clone(), pending_unregister("first"));
        later_clean.record("child", later.clone(), ChildShutdown::Clean);
        later_clean
            .settle_pending_unregisters(std::time::Instant::now())
            .await;
        assert!(later_clean.clean_in("child", &earlier));
        assert!(
            later_clean.clean_in("child", &later),
            "the later incarnation's clean receipt stays"
        );
        assert_eq!(
            later_clean.outcome(1),
            ChildMobStandDownOutcome::HeldUntilProcessExit { child_mobs: 1 }
        );
    }

    #[test]
    fn a_child_root_that_is_a_file_is_unavailable() {
        let dir = tempfile::tempdir().expect("state dir");
        let root = child_mob_root(dir.path());
        std::fs::write(&root, b"not a directory").expect("occupy the root path");
        assert!(matches!(
            ChildMobRootClaim::acquire(&root),
            Err(ChildMobStorageError::RootUnavailable { .. })
        ));
    }

    #[test]
    fn enumeration_lists_only_child_databases() {
        let dir = tempfile::tempdir().expect("state dir");
        let root = child_mob_root(dir.path());
        let _claim = ChildMobRootClaim::acquire(&root).expect("claim");
        for name in ["mob--a.db", "realm_profiles.db", "notes.txt"] {
            std::fs::write(root.join(name), b"").expect("seed file");
        }
        assert_eq!(
            child_mob_databases(dir.path()),
            vec![root.join("mob--a.db"), root.join("realm_profiles.db")]
        );
    }

    #[test]
    fn a_council_store_that_cannot_be_created_is_required() {
        let dir = tempfile::tempdir().expect("state dir");
        // A regular file where the state dir's parent should be: no store
        // can create a directory beneath it, so the council file cannot exist.
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").expect("blocker file");
        let layout = MetaStorageLayout::with_injected_roots(blocker.join("nested"), None);
        assert!(matches!(
            open_required_council_store(&layout),
            Err(ChildMobStorageError::CouncilStoreUnavailable { .. })
        ));
    }
}

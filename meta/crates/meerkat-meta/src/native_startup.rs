//! The checks every persistent Meta composition runs on its state directory
//! after claiming it ([`crate::state_dir_claim`]) and before it opens any
//! store, in this order (a storage provider's runtime store is checked once
//! the provider has opened its realm, see the third):
//!
//! 1. No other process hosts runtimes there. Core holds a shared lock on
//!    `runtime.sqlite.execution-custody` for each runtime it hosts, so an
//!    exclusive try-lock tells; the probe opens the file read-only, never
//!    creates it, and releases the lock at once. A 0.9.3 process holds no
//!    `meta-runtime.lock`, so this is the check that sees it. A shut-down
//!    predecessor this process composed keeps that lock while its handles
//!    are still in scope, and the state directory claim admits a successor
//!    in the same process then ([`crate::state_dir_claim`]). So when the
//!    try-lock is refused while a runtime store this process composed in
//!    the directory is still alive, the probe reads the kernel's lock table
//!    (`/proc/locks`, Linux) and refuses when it names any other holder of
//!    the lock. Where no holder can be named (macOS and Windows have no such
//!    table; on Linux it may be unreadable, or have no entry the probe can
//!    match, as on btrfs subvolumes and overlayfs), the probe proceeds while
//!    this process's own store is alive: every 0.9.4 start takes the state
//!    directory claim and runs this probe, so no other 0.9.4 runtime can
//!    hold the lock then. A holder that skips both (an older release, or a
//!    Core-only tool on the directory) is not detected there.
//! 2. The layout is native ([`native_layout_status`]). A directory that
//!    needs [`NATIVE_STORAGE_MIGRATION_VERB`] is refused before the
//!    head-canonical open would pin a file the migration still has to
//!    convert.
//! 3. Identity bindings still have their runtime store: identities bound to
//!    a session, by a continuity record or by Core's durable member binding
//!    in the composition's mob store, while `runtime.sqlite` is missing or
//!    holds no session are refused; restoring the runtime store is the
//!    remedy. A composition whose runtime store is a storage provider's
//!    backend ([`RuntimeStoreLocation::StorageProvider`]) has no
//!    `runtime.sqlite` here to compare with, so the claim skips this file
//!    check; the builder runs `refuse_lost_runtime_store` instead, through
//!    the provider's runtime store, after the provider has opened its realm
//!    (on `persistent_state` and `scratch_dir` realms alike).
//!
//! The database probes read with SQLite `mode=ro` (never `immutable=1`,
//! which would not see the write-ahead log) and fail closed.
//!
//! [`NATIVE_STORAGE_MIGRATION_VERB`]: crate::storage_layout::NATIVE_STORAGE_MIGRATION_VERB

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};

use crate::storage_layout::{
    CANONICAL_CONTINUITY_DB_FILE_NAME, DatabaseSlot, MetaStorageLayout, RUNTIME_DB_FILE_NAME,
    StorageLayoutError,
};
use crate::storage_migrate_native::{NativeLayoutStatus, native_layout_status};

/// Suffix of Core's execution-custody lock file beside its runtime database.
const RUNTIME_EXECUTION_CUSTODY_SUFFIX: &str = ".execution-custody";

/// The runtime stores this process composed, by canonical state directory,
/// held weakly. Core's execution claim on a store's database lives in the
/// machine and drivers that hold the store, so while one of them is still
/// alive, a held custody lock may be this process's own.
type OwnRuntimeStores = HashMap<PathBuf, Vec<Weak<dyn meerkat_runtime::RuntimeStore>>>;

static OWN_RUNTIME_STORES: LazyLock<Mutex<OwnRuntimeStores>> = LazyLock::new(Default::default);

/// Record `store`, the runtime store just opened in `state_dir`, as this
/// process's own. While it is alive, the custody probe attributes a held lock
/// before it refuses (see the module docs). A directory that cannot be
/// resolved is not recorded, so its successor is probed as before.
pub(crate) fn record_own_runtime_store(
    state_dir: &Path,
    store: &Arc<dyn meerkat_runtime::RuntimeStore>,
) {
    let Ok(key) = std::fs::canonicalize(state_dir) else {
        return;
    };
    let mut stores = OWN_RUNTIME_STORES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let entries = stores.entry(key).or_default();
    entries.retain(|entry| entry.strong_count() > 0);
    entries.push(Arc::downgrade(store));
}

/// Whether a runtime store this process composed in `state_dir` is still
/// alive. Entries whose store dropped are forgotten.
fn own_runtime_store_alive(state_dir: &Path) -> bool {
    let Ok(key) = std::fs::canonicalize(state_dir) else {
        return false;
    };
    let mut stores = OWN_RUNTIME_STORES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let Some(entries) = stores.get_mut(&key) else {
        return false;
    };
    entries.retain(|entry| entry.strong_count() > 0);
    if entries.is_empty() {
        stores.remove(&key);
        return false;
    }
    true
}

/// Refuse `state_dir` unless it is ready for this runtime (see the module
/// docs). Call it after claiming the directory and before opening any store
/// in it. `mob_store` is the composition's durable mob store, the path its
/// mob storage provenance names; it is `None` only for mob storage declared
/// in-memory, which keeps no binding across processes. The checks read files
/// and SQLite databases in place, once, at startup, before the runtime serves
/// anything.
///
/// # Errors
///
/// [`StorageLayoutError::RuntimeInUse`] or
/// [`StorageLayoutError::RuntimeLockUnreadable`] for the custody probe,
/// [`StorageLayoutError::NativeStorageMigrationRequired`] for a layout that
/// is not native, [`StorageLayoutError::RuntimeStoreMissing`] for bindings
/// without their runtime store, and
/// [`StorageLayoutError::StartupProbeUnreadable`] when a probed database,
/// the mob store included, cannot be read.
pub async fn refuse_unready_state_dir(
    state_dir: &Path,
    mob_store: Option<&Path>,
) -> Result<(), StorageLayoutError> {
    refuse_unready_state_dir_for(state_dir, mob_store, RuntimeStoreLocation::StateDir).await
}

/// Where a composition's runtime store lives, which decides whether the
/// bindings check (the third, see the module docs) has a `runtime.sqlite` to
/// read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStoreLocation {
    /// `runtime.sqlite` under the state directory: the disk composition.
    StateDir,
    /// A non-disk storage provider's meerkat-level bundle: sessions and
    /// runtime authority live in the provider's backend, and no
    /// `runtime.sqlite` exists under the state directory, so the claim skips
    /// the file check; the builder checks the bindings against the
    /// provider's runtime store (`refuse_lost_runtime_store`) once the
    /// provider has opened its realm.
    StorageProvider,
}

/// [`refuse_unready_state_dir`] for a composition whose runtime store lives
/// at `runtime_store`.
///
/// # Errors
///
/// As [`refuse_unready_state_dir`]; with
/// [`RuntimeStoreLocation::StorageProvider`] never
/// [`StorageLayoutError::RuntimeStoreMissing`].
pub async fn refuse_unready_state_dir_for(
    state_dir: &Path,
    mob_store: Option<&Path>,
    runtime_store: RuntimeStoreLocation,
) -> Result<(), StorageLayoutError> {
    refuse_hosted_runtime(state_dir)?;
    match native_layout_status(state_dir) {
        NativeLayoutStatus::Native => {}
        status => {
            return Err(StorageLayoutError::NativeStorageMigrationRequired {
                state_dir: state_dir.to_path_buf(),
                status,
            });
        }
    }
    if runtime_store == RuntimeStoreLocation::StorageProvider {
        return Ok(());
    }
    let bound_identities = bound_identity_count(state_dir, mob_store).await?;
    if bound_identities > 0 && runtime_store_is_empty(state_dir)? {
        return Err(StorageLayoutError::RuntimeStoreMissing {
            state_dir: state_dir.to_path_buf(),
            bound_identities,
        });
    }
    Ok(())
}

/// The bindings check (the third, see the module docs) for a composition
/// whose runtime store is a storage provider's backend, read through the
/// store itself once the provider has opened its realm (a `persistent_state`
/// or a `scratch_dir` realm alike). Identities bound to a session, counted
/// as [`refuse_unready_state_dir`] counts them, while that store holds no
/// session at all are refused: the backend lost its data, or the store does
/// not outlive the process, and each identity would otherwise come back on a
/// session that no longer exists. A store that holds any session passes:
/// this check does not look for one identity's missing session.
///
/// # Errors
///
/// [`StorageLayoutError::RuntimeStoreLostSessions`] for bindings whose
/// provider-held runtime store holds no session,
/// [`StorageLayoutError::RuntimeStoreUnreadable`] when that store cannot be
/// read, and [`StorageLayoutError::StartupProbeUnreadable`] when the
/// continuity database or the mob store cannot be read.
pub(crate) async fn refuse_lost_runtime_store(
    state_dir: &Path,
    mob_store: Option<&Path>,
    runtime_store: &dyn meerkat_runtime::RuntimeStore,
    provider: &str,
) -> Result<(), StorageLayoutError> {
    let bound_identities = bound_identity_count(state_dir, mob_store).await?;
    if bound_identities == 0 {
        return Ok(());
    }
    let holds_a_session = runtime_store
        .list_runtime_session_catalog_entries(meerkat_core::SessionFilter {
            limit: Some(1),
            ..meerkat_core::SessionFilter::default()
        })
        .await
        .map(|entries| !entries.is_empty())
        .map_err(|error| error.to_string());
    lost_runtime_store_verdict(state_dir, provider, bound_identities, holds_a_session)
}

/// The decision of [`refuse_lost_runtime_store`] once `bound_identities`
/// (more than zero) and the catalog probe are known: whether the provider's
/// runtime store holds a session, or why it could not be read.
fn lost_runtime_store_verdict(
    state_dir: &Path,
    provider: &str,
    bound_identities: usize,
    holds_a_session: Result<bool, String>,
) -> Result<(), StorageLayoutError> {
    match holds_a_session {
        Err(detail) => Err(StorageLayoutError::RuntimeStoreUnreadable {
            state_dir: state_dir.to_path_buf(),
            provider: provider.to_string(),
            detail,
        }),
        Ok(true) => Ok(()),
        Ok(false) => Err(StorageLayoutError::RuntimeStoreLostSessions {
            state_dir: state_dir.to_path_buf(),
            bound_identities,
            provider: provider.to_string(),
        }),
    }
}

/// Refuse a layout that names a separate session database (a `store_path`
/// with a file extension that is not an existing directory, see
/// [`MetaStorageLayout::standalone_from_store_path`]). Sessions live in
/// `runtime.sqlite` under the state directory, so the override names nothing
/// this runtime opens; the refusal names the steps that move such a file
/// into the layout. Call it right after building the layout, before any
/// store opens, whether or not the file exists.
///
/// # Errors
///
/// [`StorageLayoutError::SessionDatabaseOverrideRetired`] when the layout
/// carries a session-database override.
pub fn refuse_session_database_override(
    layout: &MetaStorageLayout,
) -> Result<(), StorageLayoutError> {
    match layout.session_db_override() {
        Some(path) => Err(StorageLayoutError::SessionDatabaseOverrideRetired {
            path: path.to_path_buf(),
            state_dir: layout.state_dir().to_path_buf(),
        }),
        None => Ok(()),
    }
}

/// Refuse a directory on which another process hosts runtimes.
fn refuse_hosted_runtime(state_dir: &Path) -> Result<(), StorageLayoutError> {
    let mut custody = state_dir.join(RUNTIME_DB_FILE_NAME).into_os_string();
    custody.push(RUNTIME_EXECUTION_CUSTODY_SUFFIX);
    let file = PathBuf::from(custody);
    let unreadable = |detail: String| StorageLayoutError::RuntimeLockUnreadable {
        state_dir: state_dir.to_path_buf(),
        file: file.clone(),
        detail,
    };
    let lock = match std::fs::File::open(&file) {
        Ok(lock) => lock,
        // No custody file has no holder; the probe never creates one.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(unreadable(error.to_string())),
    };
    match lock.try_lock() {
        Ok(()) => Ok(()),
        // A shut-down predecessor this process composed keeps Core's shared
        // lock until its last holder drops. Its state directory claim ended
        // with its shutdown, so the successor that just claimed the
        // directory proceeds unless another holder is named.
        Err(std::fs::TryLockError::WouldBlock)
            if own_runtime_store_alive(state_dir) && !another_holder_named(&lock) =>
        {
            Ok(())
        }
        Err(std::fs::TryLockError::WouldBlock) => Err(StorageLayoutError::RuntimeInUse {
            state_dir: state_dir.to_path_buf(),
            file,
        }),
        Err(std::fs::TryLockError::Error(error)) => Err(unreadable(error.to_string())),
    }
}

/// Whether the kernel's lock table (`/proc/locks`) names a holder of
/// `lock`'s file other than this process. The kernel names a `flock` holder
/// by the process that took the lock, and a process this one cannot see as
/// 0. `false` when no holder can be named: the table cannot be read, or has
/// no entry for the file, because `stat` reports another device or inode
/// than the lock's (btrfs subvolumes, overlayfs).
#[cfg(target_os = "linux")]
fn another_holder_named(lock: &std::fs::File) -> bool {
    let Some(file_id) = lock_table_file_id(lock) else {
        return false;
    };
    read_lock_table().is_some_and(|table| {
        lock_table_names_another_holder(&table, &file_id, &std::process::id().to_string())
    })
}

/// How the kernel's lock table names `file`: device major and minor (hex)
/// and inode, `<major>:<minor>:<inode>`, decoded from `stat` as glibc does.
#[cfg(target_os = "linux")]
fn lock_table_file_id(file: &std::fs::File) -> Option<String> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = file.metadata().ok()?;
    let device = metadata.dev();
    let major = ((device >> 8) & 0xfff) | ((device >> 32) & 0xffff_f000);
    let minor = (device & 0xff) | ((device >> 12) & 0xffff_ff00);
    Some(format!("{major:02x}:{minor:02x}:{}", metadata.ino()))
}

/// The kernel's lock table, `None` when it cannot be read. A test on this
/// thread may supply the table instead (see `tests::with_lock_table`).
#[cfg(target_os = "linux")]
fn read_lock_table() -> Option<String> {
    #[cfg(test)]
    if let Some(table) = tests::LOCK_TABLE.with(|table| table.borrow().clone()) {
        return Some(table);
    }
    std::fs::read_to_string("/proc/locks").ok()
}

/// No kernel table of lock owners: no other holder can be named. Every
/// 0.9.4 start takes the state directory claim and runs this probe, so no
/// other 0.9.4 runtime can hold the lock while this process's own store is
/// alive. A holder that skips both (an older release, or a Core-only tool)
/// is not detected here.
#[cfg(not(target_os = "linux"))]
fn another_holder_named(_lock: &std::fs::File) -> bool {
    false
}

/// Whether `table`, in the `/proc/locks` format, names a holder of the file
/// `file_id` (`<major>:<minor>:<inode>`) other than `this_process`.
#[cfg(any(target_os = "linux", test))]
fn lock_table_names_another_holder(table: &str, file_id: &str, this_process: &str) -> bool {
    // "<n>: FLOCK ADVISORY READ <pid> <major>:<minor>:<inode> <start> <end>".
    // A waiter's line has "->" after the index; a waiter holds nothing.
    table
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace().skip(1);
            if fields.next()? == "->" {
                return None;
            }
            let mut fields = fields.skip(2);
            let pid = fields.next()?;
            (fields.next()? == file_id).then_some(pid)
        })
        .any(|pid| pid != this_process)
}

/// How many distinct identities are bound to a session: by a row of
/// `continuity_records` that names one, or by Core's durable member binding
/// in `mob_store` (each durable identity's last binding since the latest mob
/// reset, live or retired) that names one. A missing database, table or mob
/// store binds none.
async fn bound_identity_count(
    state_dir: &Path,
    mob_store: Option<&Path>,
) -> Result<usize, StorageLayoutError> {
    let names = std::iter::once(CANONICAL_CONTINUITY_DB_FILE_NAME)
        .chain(DatabaseSlot::Continuity.legacy_names().iter().copied());
    let mut bound = BTreeSet::new();
    for name in names {
        let path = state_dir.join(name);
        bound.extend(read_only_identities(
            state_dir,
            &path,
            "continuity_records",
            "SELECT identity FROM continuity_records WHERE session_id <> ''",
        )?);
    }
    if let Some(mob_store) = mob_store {
        bound.extend(mob_store_bound_identities(state_dir, mob_store).await?);
    }
    Ok(bound.len())
}

/// The durable identities whose last binding in the mob store at `path`
/// names a session, read through Core's read-only member history reader (it
/// never creates, migrates or writes the store). Core names a member by its
/// roster id, the comms-safe encoding of the identity, so each is decoded to
/// the identity a continuity record names: an identity bound both ways counts
/// once. A missing store binds none; a store this build cannot read refuses.
async fn mob_store_bound_identities(
    state_dir: &Path,
    path: &Path,
) -> Result<Vec<String>, StorageLayoutError> {
    match meerkat_mob::MobStorage::read_member_binding_history(path).await {
        Ok(history) => Ok(history
            .durable_identity_bindings()
            .filter(|binding| binding.session_id().is_some())
            .map(|binding| {
                crate::member_comms_id::durable_identity_for_roster_member(
                    binding.identity().as_ref(),
                )
            })
            .collect()),
        Err(meerkat_mob::MemberBindingHistoryReadError::StoreMissing { .. }) => Ok(Vec::new()),
        Err(error) => Err(unreadable(state_dir, path, error.to_string())),
    }
}

/// Whether `runtime.sqlite` holds no session: the file is absent, or neither
/// `session_heads` nor `sessions` has a row (a missing table holds none).
fn runtime_store_is_empty(state_dir: &Path) -> Result<bool, StorageLayoutError> {
    let path = state_dir.join(RUNTIME_DB_FILE_NAME);
    let heads = read_only_count(
        state_dir,
        &path,
        "session_heads",
        "SELECT COUNT(*) FROM session_heads",
    )?;
    let sessions = read_only_count(
        state_dir,
        &path,
        "sessions",
        "SELECT COUNT(*) FROM sessions",
    )?;
    Ok(heads == 0 && sessions == 0)
}

/// A read-only connection to `path` when the file and `table` exist; `None`
/// otherwise. Errors refuse.
fn open_read_only_table(
    state_dir: &Path,
    path: &Path,
    table: &str,
) -> Result<Option<rusqlite::Connection>, StorageLayoutError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(state_dir, path, error.to_string())),
    }
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| unreadable(state_dir, path, error.to_string()))?;
    let present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(|error| unreadable(state_dir, path, error.to_string()))?;
    Ok(present.then_some(conn))
}

/// Run `count_sql` over a read-only connection to `path` when the file and
/// `table` exist; zero otherwise. Errors refuse.
fn read_only_count(
    state_dir: &Path,
    path: &Path,
    table: &str,
    count_sql: &str,
) -> Result<usize, StorageLayoutError> {
    let Some(conn) = open_read_only_table(state_dir, path, table)? else {
        return Ok(0);
    };
    let count: i64 = conn
        .query_row(count_sql, [], |row| row.get(0))
        .map_err(|error| unreadable(state_dir, path, error.to_string()))?;
    usize::try_from(count).map_err(|error| unreadable(state_dir, path, error.to_string()))
}

/// The first column of every row `sql` selects over a read-only connection
/// to `path` when the file and `table` exist; none otherwise. Errors refuse.
fn read_only_identities(
    state_dir: &Path,
    path: &Path,
    table: &str,
    sql: &str,
) -> Result<Vec<String>, StorageLayoutError> {
    let Some(conn) = open_read_only_table(state_dir, path, table)? else {
        return Ok(Vec::new());
    };
    let mut statement = conn
        .prepare(sql)
        .map_err(|error| unreadable(state_dir, path, error.to_string()))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| unreadable(state_dir, path, error.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| unreadable(state_dir, path, error.to_string()))
}

fn unreadable(state_dir: &Path, path: &Path, detail: String) -> StorageLayoutError {
    StorageLayoutError::StartupProbeUnreadable {
        state_dir: state_dir.to_path_buf(),
        file: path.to_path_buf(),
        detail,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::io::{BufRead as _, Write as _};

    use super::*;
    use crate::storage_layout::STATE_DIR_LOCK_FILE_NAME;

    /// Names directly under `dir`.
    fn entries(dir: &Path) -> BTreeSet<String> {
        std::fs::read_dir(dir)
            .expect("read the state directory")
            .map(|entry| {
                entry
                    .expect("directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    fn custody_file(state_dir: &Path) -> PathBuf {
        let mut custody = state_dir.join(RUNTIME_DB_FILE_NAME).into_os_string();
        custody.push(RUNTIME_EXECUTION_CUSTODY_SUFFIX);
        PathBuf::from(custody)
    }

    fn definition() -> meerkat_mob::MobDefinition {
        meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"native-startup\"\n")
            .expect("minimal mob definition")
    }

    /// A persistent composition over `state_dir`, the path every builder and
    /// constructor caller takes.
    async fn compose(
        state_dir: &Path,
    ) -> Result<crate::MobBootstrapSpec, crate::storage_health::StorageResolutionError> {
        crate::MobBootstrapSpec::persistent(
            definition(),
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            state_dir.to_path_buf(),
            4,
        )
        .await
    }

    /// A continuity database binding one identity to a session, written
    /// through the store's own schema.
    fn bind_one_identity(state_dir: &Path) {
        bind_identity(state_dir, "agent-a");
    }

    /// A continuity record binding `identity` to a session, written through
    /// the store's own schema.
    fn bind_identity(state_dir: &Path, identity: &str) {
        let path = state_dir.join(CANONICAL_CONTINUITY_DB_FILE_NAME);
        drop(
            crate::identity_first::LocalContinuityStore::open(&path)
                .expect("open the continuity store"),
        );
        let conn = rusqlite::Connection::open(&path).expect("open the continuity database");
        conn.execute(
            "INSERT INTO continuity_records \
             (identity, agent_runtime_id, session_id, generation, checkpoint_version, fencing_token) \
             VALUES (?1, ?2, ?3, 0, 0, 1)",
            [
                identity.to_string(),
                format!("rt:{identity}:0"),
                meerkat_core::types::SessionId::new().to_string(),
            ],
        )
        .expect("bind one identity");
    }

    const CUSTODY_HOLDER_CHILD: &str = "META_TEST_RUNTIME_CUSTODY_HOLDER";
    const CUSTODY_HOLDER_TEST: &str =
        "native_startup::tests::a_directory_another_process_hosts_refuses_before_any_store_opens";

    /// Custody probe: while another process holds a shared lock on
    /// `runtime.sqlite.execution-custody` (as Core does for each runtime it
    /// hosts, 0.9.3 included), a persistent composition refuses typed
    /// (`RuntimeInUse`, naming the file) and opens no store: only the state
    /// directory's own lock file may appear. Control: once the holder exits,
    /// the same directory composes.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_directory_another_process_hosts_refuses_before_any_store_opens() {
        if let Some(path) = std::env::var_os(CUSTODY_HOLDER_CHILD) {
            // The child: hold a shared lock on the custody file until the
            // parent closes stdin.
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .expect("open the custody file");
            file.lock_shared().expect("take the shared custody lock");
            println!("custody-held");
            std::io::stdout().flush().expect("flush stdout");
            let mut sink = String::new();
            let _ = std::io::stdin().read_line(&mut sink);
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let custody = custody_file(dir.path());
        let mut holder = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                CUSTODY_HOLDER_TEST,
                "--exact",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env(CUSTODY_HOLDER_CHILD, &custody)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn the custody holder");
        // The holder's lines arrive through a thread, so the wait for its
        // marker is bounded: a holder that never takes the lock fails the
        // test instead of hanging it.
        let stdout = holder.stdout.take().expect("holder stdout");
        let (lines_tx, lines_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if lines_tx.send(line).is_err() {
                    break;
                }
            }
        });
        loop {
            let line = lines_rx
                .recv_timeout(std::time::Duration::from_mins(1))
                .expect("the holder took the lock within 60 s");
            // With one test thread, libtest prints the test's name and its
            // output on one line.
            if line.contains("custody-held") {
                break;
            }
        }
        let before = entries(dir.path());
        let probing = std::time::Instant::now();
        match compose(dir.path()).await {
            Err(crate::storage_health::StorageResolutionError::Layout(
                StorageLayoutError::RuntimeInUse { state_dir, file },
            )) => {
                assert_eq!(state_dir, dir.path());
                assert_eq!(file, custody);
                // The probe is one try-lock: the refusal is prompt.
                let waited = probing.elapsed();
                assert!(
                    waited < std::time::Duration::from_secs(5),
                    "refused after {waited:?}"
                );
            }
            Err(other) => panic!("expected RuntimeInUse, got: {other}"),
            Ok(_) => panic!("a directory another process hosts must refuse"),
        }
        let added = entries(dir.path())
            .difference(&before)
            .filter(|name| name.as_str() != STATE_DIR_LOCK_FILE_NAME)
            .cloned()
            .collect::<Vec<_>>();
        assert!(added.is_empty(), "the refusal opened stores: {added:?}");

        drop(holder.stdin.take());
        let status = holder.wait().expect("wait for the holder");
        assert!(status.success(), "the custody holder failed: {status}");
        compose(dir.path())
            .await
            .expect("the same directory composes once the holder exits");
    }

    /// Mob storage handed over without a provenance refuses typed before the
    /// state directory is claimed: the composition cannot tell whether its
    /// mob store outlives the process. Control: the same directory composes
    /// once the storage is declared in-memory.
    #[tokio::test]
    async fn undeclared_mob_storage_refuses_before_the_directory_is_claimed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = crate::MobBootstrapSpec::persistent(
            definition(),
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::default(),
            dir.path().to_path_buf(),
            4,
        )
        .await;
        match result {
            Err(crate::storage_health::StorageResolutionError::Layout(
                StorageLayoutError::MobStorageUndeclared { state_dir },
            )) => assert_eq!(state_dir, dir.path()),
            Err(other) => panic!("expected MobStorageUndeclared, got: {other}"),
            Ok(_) => panic!("undeclared mob storage must refuse"),
        }
        assert!(
            entries(dir.path()).is_empty(),
            "the refusal touched the state directory"
        );
        compose(dir.path())
            .await
            .expect("the same directory composes with declared mob storage");
    }

    /// The custody probe never creates the custody file: a fresh directory
    /// passes and is left without one.
    #[tokio::test]
    async fn the_custody_probe_creates_nothing_in_a_fresh_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        refuse_unready_state_dir(dir.path(), None)
            .await
            .expect("a fresh directory is ready");
        assert!(entries(dir.path()).is_empty(), "the checks wrote files");
        assert!(!custody_file(dir.path()).exists());
    }

    /// A session row still only in the write-ahead log counts: a writer that
    /// committed it is open and has checkpointed nothing, as after an
    /// unclean stop. The probe reads with `mode=ro`, which reads the log, so
    /// the bindings start. Control: an `immutable=1` read of the same file,
    /// which skips the log, sees no session row, which is why the probe never
    /// uses it.
    #[tokio::test]
    async fn sessions_still_only_in_the_write_ahead_log_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        bind_one_identity(dir.path());
        let path = dir.path().join(RUNTIME_DB_FILE_NAME);
        // The session store's own schema, checkpointed when its connection
        // closes, and no session yet.
        drop(
            meerkat_store::SqliteSessionStore::open(path.clone())
                .expect("the runtime database's session store"),
        );
        let writer = rusqlite::Connection::open(&path).expect("a writer on the runtime database");
        writer
            .execute_batch(
                "PRAGMA wal_autocheckpoint=0; \
                 INSERT INTO sessions (session_id, created_at_ms, updated_at_ms, message_count, \
                 total_tokens, metadata_json, session_json) \
                 VALUES ('wal-only', 0, 0, 0, 0, '{}', x'00');",
            )
            .expect("commit one session row into the write-ahead log");
        let main_file_sessions: i64 = {
            let uri = format!("file:{}?immutable=1", path.display());
            let conn = rusqlite::Connection::open_with_flags(
                &uri,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )
            .expect("an immutable read of the main file");
            conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
                .expect("count the main file's session rows")
        };
        assert_eq!(
            main_file_sessions, 0,
            "the session row must still be only in the write-ahead log"
        );
        refuse_unready_state_dir(dir.path(), None)
            .await
            .expect("a session row in the write-ahead log is a session");
        drop(writer);
    }

    /// A composition whose runtime store is a storage provider's backend has
    /// no `runtime.sqlite` under the state directory, so its bindings are not
    /// compared with one: a bound identity there starts, and nothing is
    /// created. Control: the same directory refuses for the disk
    /// composition, whose runtime store is that missing `runtime.sqlite`.
    #[tokio::test]
    async fn a_storage_provider_runtime_store_skips_the_bindings_check() {
        let dir = tempfile::tempdir().expect("tempdir");
        bind_one_identity(dir.path());
        assert!(matches!(
            refuse_unready_state_dir_for(dir.path(), None, RuntimeStoreLocation::StateDir).await,
            Err(StorageLayoutError::RuntimeStoreMissing {
                bound_identities: 1,
                ..
            })
        ));
        refuse_unready_state_dir_for(dir.path(), None, RuntimeStoreLocation::StorageProvider)
            .await
            .expect("a provider-held runtime store leaves nothing here to compare with");
        assert!(!dir.path().join(RUNTIME_DB_FILE_NAME).exists());
    }

    /// Identity bindings without their runtime store refuse typed, naming
    /// how many identities are bound, whether `runtime.sqlite`
    /// is missing or holds no session. Bindings whose runtime store holds a
    /// session start, and so does a directory with no bindings.
    #[tokio::test]
    async fn identity_bindings_without_their_runtime_store_refuse() {
        let missing = tempfile::tempdir().expect("tempdir");
        bind_one_identity(missing.path());
        assert!(matches!(
            refuse_unready_state_dir(missing.path(), None).await,
            Err(StorageLayoutError::RuntimeStoreMissing {
                bound_identities: 1,
                ..
            })
        ));

        let empty = tempfile::tempdir().expect("tempdir");
        bind_one_identity(empty.path());
        drop(
            meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(
                empty.path().join(RUNTIME_DB_FILE_NAME),
            )
            .expect("an empty runtime store"),
        );
        assert!(matches!(
            refuse_unready_state_dir(empty.path(), None).await,
            Err(StorageLayoutError::RuntimeStoreMissing {
                bound_identities: 1,
                ..
            })
        ));

        let held = tempfile::tempdir().expect("tempdir");
        bind_one_identity(held.path());
        let sessions =
            meerkat_store::SqliteSessionStore::open(held.path().join(RUNTIME_DB_FILE_NAME))
                .expect("the runtime database's session store");
        meerkat::SessionStore::save(&sessions, &meerkat_core::Session::new())
            .await
            .expect("save one session");
        drop(sessions);
        refuse_unready_state_dir(held.path(), None)
            .await
            .expect("bindings with their sessions start");

        let fresh = tempfile::tempdir().expect("tempdir");
        refuse_unready_state_dir(fresh.path(), None)
            .await
            .expect("a directory without bindings starts");
    }

    /// A storage provider's runtime store that holds no session while an
    /// identity is bound to one refuses typed, naming the binding count and
    /// the provider.
    #[tokio::test]
    async fn bindings_without_a_session_in_the_provider_runtime_store_refuse() {
        let dir = tempfile::tempdir().expect("tempdir");
        bind_one_identity(dir.path());
        let empty = meerkat_runtime::InMemoryRuntimeStore::new();
        let refused = refuse_lost_runtime_store(dir.path(), None, &empty, "stub").await;
        assert!(
            matches!(
                refused,
                Err(StorageLayoutError::RuntimeStoreLostSessions {
                    bound_identities: 1,
                    ref provider,
                    ..
                }) if provider == "stub"
            ),
            "{refused:?}"
        );
    }

    /// A provider runtime store whose catalog cannot be read refuses typed,
    /// naming the provider and the backend's error: an unreadable store is
    /// no evidence either way, so the build does not start over it.
    #[test]
    fn an_unreadable_provider_runtime_store_refuses() {
        let refused = lost_runtime_store_verdict(
            Path::new("/state"),
            "stub",
            1,
            Err("backend unreachable".to_string()),
        );
        assert!(
            matches!(
                refused,
                Err(StorageLayoutError::RuntimeStoreUnreadable {
                    ref provider,
                    ref detail,
                    ..
                }) if provider == "stub" && detail == "backend unreachable"
            ),
            "{refused:?}"
        );
    }

    /// Without bindings, an empty provider runtime store starts: a fresh
    /// installation and an in-memory substrate bind nothing yet.
    #[tokio::test]
    async fn no_bindings_start_over_an_empty_provider_runtime_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let empty = meerkat_runtime::InMemoryRuntimeStore::new();
        refuse_lost_runtime_store(dir.path(), None, &empty, "stub")
            .await
            .expect("a directory without bindings starts");
    }

    /// A mob with one turn-driven profile, for a durable identity spawn.
    fn durable_member_definition() -> meerkat_mob::MobDefinition {
        meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"native-startup-bindings\"\n\n[profiles.lead]\nmodel = \"gpt-5.5\"\n\
             runtime_mode = \"turn_driven\"\n\n[profiles.lead.tools]\ncomms = true\n",
        )
        .expect("durable member definition")
    }

    /// The bindings check counts Core's durable member bindings too. A
    /// durable identity spawned through Core (a 0.9.4-born identity, with no
    /// continuity record at all) whose binding names a session refuses startup
    /// typed once `runtime.sqlite` is gone, instead of waking fresh. Control:
    /// the same directory starts while `runtime.sqlite` still holds the
    /// session. Core names the member by its encoded roster id; an identity
    /// that a continuity record binds as well counts once.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_core_member_binding_without_its_runtime_store_refuses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path();
        let mob_store = state.join(crate::storage_layout::EVENT_LOG_DB_FILE_NAME);
        {
            let (storage, provenance) =
                crate::mob_composition_manifest::persistent_mob_storage(mob_store.clone())
                    .expect("persistent mob storage");
            let mut spec = crate::MobBootstrapSpec::persistent(
                durable_member_definition(),
                storage,
                provenance,
                state.to_path_buf(),
                4,
            )
            .await
            .expect("persistent composition");
            spec.options.default_llm_client =
                Some(std::sync::Arc::new(meerkat_client::TestClient::default()));
            let runtime = Box::pin(crate::MobRuntime::bootstrap(spec))
                .await
                .expect("bootstrap the mob runtime");
            // The roster id Meta spawns a durable identity under.
            let member_id = crate::member_comms_id::mob_member_id("domain:durable-member");
            let mut member = meerkat_mob::SpawnMemberSpec::new("lead", member_id.clone());
            member.continuity_intent = meerkat_mob::SpawnContinuityIntent::DurableIdentity {
                continuity_key: "domain:durable-member".to_string(),
            };
            Box::pin(runtime.handle().ensure_member(member))
                .await
                .expect("spawn the durable identity");
            runtime
                .handle()
                .shutdown()
                .await
                .expect("shut the mob down");
        }
        assert_eq!(
            bound_identity_count(state, None)
                .await
                .expect("count continuity bindings"),
            0,
            "no continuity record binds the identity"
        );
        assert_eq!(
            bound_identity_count(state, Some(&mob_store))
                .await
                .expect("count Core bindings"),
            1
        );
        bind_identity(state, "domain:durable-member");
        assert_eq!(
            bound_identity_count(state, Some(&mob_store))
                .await
                .expect("count both bindings"),
            1,
            "an identity bound both ways counts once"
        );
        refuse_unready_state_dir(state, Some(&mob_store))
            .await
            .expect("the binding's runtime store holds its session");

        for suffix in ["", "-wal", "-shm"] {
            let mut path = state.join(RUNTIME_DB_FILE_NAME).into_os_string();
            path.push(suffix);
            match std::fs::remove_file(PathBuf::from(path)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove the runtime store: {error}"),
            }
        }
        assert!(matches!(
            refuse_unready_state_dir(state, Some(&mob_store)).await,
            Err(StorageLayoutError::RuntimeStoreMissing {
                bound_identities: 1,
                ..
            })
        ));
    }

    /// An unmigrated directory refuses typed before any store opens, naming
    /// the migration verb, and the refusal leaves every file byte-identical:
    /// its 0.9.3 WholeBlob runtime store takes no head-canonical pin, and
    /// still opens as a 0.9.3 store afterwards, so `storage-migrate-native`
    /// can still convert it.
    #[tokio::test]
    async fn an_unmigrated_directory_refuses_and_is_left_byte_identical() {
        use meerkat_runtime::RuntimeStore as _;
        let dir = tempfile::tempdir().expect("tempdir");
        drop(
            meerkat_store::SqliteSessionStore::open(dir.path().join("sessions.sqlite3"))
                .expect("a 0.9.3 sessions database"),
        );
        let runtime = dir.path().join(RUNTIME_DB_FILE_NAME);
        let session = meerkat_core::Session::new();
        let whole_blob = meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(&runtime)
            .expect("a 0.9.3 runtime store");
        whole_blob
            .commit_session_snapshot(
                &meerkat_runtime::LogicalRuntimeId::for_session(session.id()),
                meerkat_runtime::store::SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&session).expect("encode").into(),
                },
            )
            .await
            .expect("a committed WholeBlob body");
        drop(whole_blob);
        let snapshot = |dir: &Path| {
            std::fs::read_dir(dir)
                .expect("read the state directory")
                .map(|entry| {
                    let entry = entry.expect("directory entry");
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        std::fs::read(entry.path()).expect("read a state file"),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let before = snapshot(dir.path());
        let error = match compose(dir.path()).await {
            Err(crate::storage_health::StorageResolutionError::Layout(
                error @ StorageLayoutError::NativeStorageMigrationRequired { .. },
            )) => error,
            Err(other) => panic!("expected NativeStorageMigrationRequired, got: {other}"),
            Ok(_) => panic!("an unmigrated directory must refuse"),
        };
        assert!(
            error
                .to_string()
                .contains(crate::storage_layout::NATIVE_STORAGE_MIGRATION_VERB),
            "{error}"
        );
        let mut after = snapshot(dir.path());
        // The claim's lock file, and the read-only probe's shared-memory
        // index and empty write-ahead log, are the only additions allowed.
        after.remove(STATE_DIR_LOCK_FILE_NAME);
        for database in ["sessions.sqlite3", RUNTIME_DB_FILE_NAME] {
            after.remove(&format!("{database}-shm"));
            let wal = format!("{database}-wal");
            if after.get(&wal).is_some_and(Vec::is_empty) {
                after.remove(&wal);
            }
        }
        assert!(before.contains_key(RUNTIME_DB_FILE_NAME));
        assert_eq!(after, before, "the refusal changed the state directory");
        drop(
            meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(&runtime)
                .expect("the refused runtime store still opens as a 0.9.3 store"),
        );
    }

    /// A layout naming a separate session database refuses typed
    /// whether or not the file exists, naming the file, the state directory
    /// and the three remedy steps in order. Control: the state directory
    /// itself is no override.
    #[test]
    fn a_session_database_override_refuses_whether_or_not_the_file_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let file = dir.path().join("my-sessions.db");
        for exists in [false, true] {
            if exists {
                std::fs::write(&file, b"").expect("create the named file");
            }
            let layout = MetaStorageLayout::standalone_from_store_path(&file, home.clone());
            let error = refuse_session_database_override(&layout)
                .expect_err("a session-database override must refuse");
            let StorageLayoutError::SessionDatabaseOverrideRetired { path, state_dir } = &error
            else {
                panic!("expected SessionDatabaseOverrideRetired (file exists: {exists}): {error}");
            };
            assert_eq!(path, &file);
            assert_eq!(state_dir, dir.path());
            let text = error.to_string();
            assert!(text.contains(&file.display().to_string()), "{text}");
            assert!(text.contains(RUNTIME_DB_FILE_NAME), "{text}");
            let import = text
                .find("meta_gateway storage-migrate --state-dir")
                .unwrap_or_else(|| panic!("the import step is missing: {text}"));
            let native = text
                .find("meta_gateway storage-migrate-native --state-dir")
                .unwrap_or_else(|| panic!("the native migration step is missing: {text}"));
            assert!(import < native, "the remedy steps are out of order: {text}");
            assert!(
                text.contains(&format!("--session-db {}", file.display())),
                "{text}"
            );
        }
        let layout = MetaStorageLayout::standalone_from_store_path(dir.path(), home);
        refuse_session_database_override(&layout).expect("the state directory is no override");
    }

    /// An existing directory is the state directory even with a dot
    /// in its name, so it is never an override; a missing dotted path still
    /// is one.
    #[test]
    fn an_existing_dotted_directory_is_the_state_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let dotted = dir.path().join("meta.d");
        std::fs::create_dir(&dotted).expect("create the dotted directory");
        let layout = MetaStorageLayout::standalone_from_store_path(&dotted, home.clone());
        assert_eq!(layout.state_dir(), dotted);
        assert_eq!(layout.session_db_override(), None);
        refuse_session_database_override(&layout).expect("an existing directory is no override");

        let missing = dir.path().join("meta.v2");
        let layout = MetaStorageLayout::standalone_from_store_path(&missing, home);
        assert!(matches!(
            refuse_session_database_override(&layout),
            Err(StorageLayoutError::SessionDatabaseOverrideRetired { .. })
        ));
    }

    /// The session facade and its write-epoch, prewarm and drain machinery
    /// are gone: Core's native pair owns session persistence, and the
    /// console reads Core's bounded boundary authority. No code in the files
    /// that composed or consumed them may name them again (a comment may,
    /// for history).
    #[test]
    fn the_retired_session_facade_stays_deleted() {
        const RETIRED: &[&str] = &[
            "SessionStoreBackedRuntimeStore",
            "epoch_tracking_runtime_store",
            "SessionSnapshotWriteEpochs",
            "SessionWriteEpochsHandle",
            "SessionDocumentReadAbsorber",
            "with_session_write_epochs",
            "runtime_authority_prewarm",
            "PrewarmPersistedAuthority",
            "mint_runtime_authority_from_durable",
            "project_committed_session_to_durable",
            "session_document_write_epoch",
            "session_drain_observation",
            "SessionDrainTracker",
        ];
        let sources = [
            (
                "mob_handle_runtime.rs",
                include_str!("mob_handle_runtime.rs"),
            ),
            ("rpc_gateway.rs", include_str!("rpc_gateway.rs")),
            ("bin/meta_gateway.rs", include_str!("bin/meta_gateway.rs")),
            (
                "unified_runtime/builder.rs",
                include_str!("unified_runtime/builder.rs"),
            ),
            (
                "unified_runtime/mod.rs",
                include_str!("unified_runtime/mod.rs"),
            ),
            (
                "console_aggregator/mod.rs",
                include_str!("console_aggregator/mod.rs"),
            ),
            (
                "console_aggregator/assistant_history_refresh.rs",
                include_str!("console_aggregator/assistant_history_refresh.rs"),
            ),
            ("storage_provider.rs", include_str!("storage_provider.rs")),
            (
                "gateway_composition.rs",
                include_str!("gateway_composition.rs"),
            ),
        ];
        let mut hits = Vec::new();
        for (file, text) in sources {
            for (index, line) in text.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                for name in RETIRED {
                    if code.contains(name) {
                        hits.push(format!("{file}:{}: {name}", index + 1));
                    }
                }
            }
        }
        assert!(
            hits.is_empty(),
            "retired session facade names are back:\n{}",
            hits.join("\n")
        );
    }

    /// While a runtime store this process composed in the directory is
    /// alive and this process is the only holder of Core's shared custody
    /// lock (a shut-down predecessor whose handles are still in scope), the
    /// probe admits the same-process successor. Once that store has dropped,
    /// the probe refuses a lock that is still held, and passes once it is
    /// released.
    #[tokio::test]
    async fn this_process_runtime_store_admits_its_successor_until_it_drops() {
        let dir = tempfile::tempdir().expect("tempdir");
        let realm = meerkat_core::RealmId::parse("custody-probe").expect("realm");
        let (sessions, runtime_store) =
            crate::mob_handle_runtime::open_native_session_pair(dir.path(), &realm)
                .expect("open the native session pair");
        let claim = runtime_store
            .execution_custody()
            .expect("the SQLite runtime store has execution custody")
            .try_acquire_shared()
            .expect("hold Core's shared custody, as a hosted runtime does");
        refuse_unready_state_dir(dir.path(), None)
            .await
            .expect("this process's own live runtime store admits the successor");

        drop(sessions);
        drop(runtime_store);
        assert!(
            matches!(
                refuse_unready_state_dir(dir.path(), None).await,
                Err(StorageLayoutError::RuntimeInUse { .. })
            ),
            "with the store dropped, a held custody lock refuses"
        );

        drop(claim);
        refuse_unready_state_dir(dir.path(), None)
            .await
            .expect("a released custody lock passes");
    }

    /// The probe attributes a held custody lock before it admits anything:
    /// while this process's predecessor holds Core's shared lock and another
    /// process holds it too, the successor is refused promptly, and admitted
    /// again once that process has let go.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn another_holder_is_refused_while_this_process_predecessor_holds_custody() {
        let dir = tempfile::tempdir().expect("tempdir");
        let realm = meerkat_core::RealmId::parse("custody-probe").expect("realm");
        let (_sessions, runtime_store) =
            crate::mob_handle_runtime::open_native_session_pair(dir.path(), &realm)
                .expect("open the native session pair");
        let _claim = runtime_store
            .execution_custody()
            .expect("the SQLite runtime store has execution custody")
            .try_acquire_shared()
            .expect("hold Core's shared custody, as a hosted runtime does");
        refuse_unready_state_dir(dir.path(), None)
            .await
            .expect("only this process holds custody so far");

        // Another process takes a shared lock beside this process's, as a
        // second runtime host would.
        let mut holder = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                CUSTODY_HOLDER_TEST,
                "--exact",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env(CUSTODY_HOLDER_CHILD, custody_file(dir.path()))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn the custody holder");
        let stdout = holder.stdout.take().expect("holder stdout");
        let (lines_tx, lines_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if lines_tx.send(line).is_err() {
                    break;
                }
            }
        });
        loop {
            let line = lines_rx
                .recv_timeout(std::time::Duration::from_mins(1))
                .expect("the holder took the lock within 60 s");
            if line.contains("custody-held") {
                break;
            }
        }

        let probing = std::time::Instant::now();
        assert!(
            matches!(
                refuse_unready_state_dir(dir.path(), None).await,
                Err(StorageLayoutError::RuntimeInUse { .. })
            ),
            "another holder refuses although this process's predecessor is alive"
        );
        let waited = probing.elapsed();
        assert!(
            waited < std::time::Duration::from_secs(5),
            "refused after {waited:?}"
        );

        drop(holder.stdin.take());
        let status = holder.wait().expect("wait for the holder");
        assert!(status.success(), "the custody holder failed: {status}");
        refuse_unready_state_dir(dir.path(), None)
            .await
            .expect("only this process holds custody again");
    }

    /// The lock table names another holder only by a non-waiting entry for
    /// the same file whose pid is not this process. No entry for the file
    /// names nobody, so the probe falls back to this process's own store.
    #[test]
    fn the_lock_table_names_another_holder_only_by_a_held_entry_for_the_file() {
        let file = "fd:01:4242";
        let own = "100";
        let table = |lines: &[&str]| lines.join("\n");
        assert!(!lock_table_names_another_holder(
            &table(&["1: FLOCK  ADVISORY  READ 100 fd:01:4242 0 EOF"]),
            file,
            own
        ));
        assert!(lock_table_names_another_holder(
            &table(&[
                "1: FLOCK  ADVISORY  READ 100 fd:01:4242 0 EOF",
                "2: FLOCK  ADVISORY  READ 200 fd:01:4242 0 EOF",
            ]),
            file,
            own
        ));
        assert!(
            lock_table_names_another_holder(
                &table(&["1: FLOCK  ADVISORY  READ 0 fd:01:4242 0 EOF"]),
                file,
                own
            ),
            "a holder this process cannot see is another"
        );
        assert!(
            !lock_table_names_another_holder(
                &table(&[
                    "1: FLOCK  ADVISORY  READ 100 fd:01:4242 0 EOF",
                    "1: -> FLOCK  ADVISORY  WRITE 200 fd:01:4242 0 EOF",
                ]),
                file,
                own
            ),
            "a waiter holds nothing"
        );
        assert!(
            !lock_table_names_another_holder(
                &table(&["1: FLOCK  ADVISORY  WRITE 200 fd:01:9999 0 EOF"]),
                file,
                own
            ),
            "no entry for the file names nobody"
        );
        assert!(!lock_table_names_another_holder("", file, own));
    }

    #[cfg(target_os = "linux")]
    thread_local! {
        /// The lock table `read_lock_table` returns on this thread, when set.
        pub(super) static LOCK_TABLE: std::cell::RefCell<Option<String>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Run the probe on this thread against `table` instead of
    /// `/proc/locks`.
    #[cfg(target_os = "linux")]
    async fn with_lock_table<T>(table: String, probe: impl std::future::Future<Output = T>) -> T {
        LOCK_TABLE.with(|current| *current.borrow_mut() = Some(table));
        let outcome = probe.await;
        LOCK_TABLE.with(|current| *current.borrow_mut() = None);
        outcome
    }

    /// Where the kernel's lock table names no holder of the custody file
    /// (btrfs subvolumes and overlayfs, whose `stat` ids are not the lock's),
    /// a same-process successor starts while this process's own store is
    /// alive. An entry for the same file that names another process still
    /// refuses.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn an_unattributed_custody_lock_admits_the_same_process_successor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let realm = meerkat_core::RealmId::parse("custody-probe").expect("realm");
        let (_sessions, runtime_store) =
            crate::mob_handle_runtime::open_native_session_pair(dir.path(), &realm)
                .expect("open the native session pair");
        let _claim = runtime_store
            .execution_custody()
            .expect("the SQLite runtime store has execution custody")
            .try_acquire_shared()
            .expect("hold Core's shared custody, as a hosted runtime does");

        // A table with no entry the probe can match to the custody file.
        let unmatched = "1: FLOCK  ADVISORY  READ 0 00:00:0 0 EOF".to_string();
        with_lock_table(unmatched, refuse_unready_state_dir(dir.path(), None))
            .await
            .expect("an unattributed lock admits the same-process successor");

        // The same table naming another process on the custody file refuses.
        let custody = std::fs::File::open(custody_file(dir.path())).expect("custody file");
        let file_id = lock_table_file_id(&custody).expect("custody file id");
        let another = format!("1: FLOCK  ADVISORY  READ 0 {file_id} 0 EOF");
        assert!(
            matches!(
                with_lock_table(another, refuse_unready_state_dir(dir.path(), None)).await,
                Err(StorageLayoutError::RuntimeInUse { .. })
            ),
            "an entry for the custody file naming another process refuses"
        );
    }
}

//! Process custody of a mob store's identity-lease incarnations.
//!
//! An identity reconcile lease names its holder (the mob) and the
//! incarnation (one actor build) that holds it. A restarted mob is a new
//! incarnation, and an unexpired lease of an earlier one makes it wait for
//! that lease to expire, because the earlier incarnation's work may still
//! run. When the earlier incarnation's process has ended, none of its work
//! runs, and the wait only keeps the identity's admission closed.
//!
//! The first actor build for a store file in a process takes an exclusive
//! descriptor lock on `<store>.incarnation-custody` and keeps it until the
//! process exits: no actor end, storage drop or Shutdown releases it. An
//! incarnation minted while its process holds the lock carries the
//! [`CUSTODY_MARK`]. So while this process holds the lock, a marked
//! incarnation it did not mint belongs to a process that has exited: that
//! process held the lock from before the mint until its exit, and the
//! operating system releases a descriptor lock only when the descriptor
//! closes. An incarnation this process minted may still have work running
//! here, so it is never taken as ended.
//!
//! The lock file is named after the store's path, which must be canonical:
//! two spellings of one store (a symlink, a relative path) would name two
//! lock files, and two processes could each hold one. The SQLite identity
//! store's path is canonical because `SqliteMobStores::open` canonicalizes
//! it and `identity_store` inherits that path; a new constructor of a store
//! that names a custody file must canonicalize as well.
//!
//! The lock file is created without truncation and never removed or
//! replaced: a removed and recreated lock file is the one way to admit a
//! second holder. A store reached through more than one link, a lock file
//! that cannot be opened or locked, and platforms other than Linux and macOS
//! mint unmarked incarnations, which every later incarnation waits out.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};

/// Prefix of an incarnation id minted while its process held the store's
/// incarnation custody. Incarnation ids are otherwise opaque: they are only
/// compared for equality.
pub(crate) const CUSTODY_MARK: &str = "custody:";

/// Suffix appended to the store file's path to name its custody lock file.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const LOCK_SUFFIX: &str = ".incarnation-custody";

#[derive(Default)]
struct ProcessCustody {
    /// Held lock descriptors by the lock file's (device, inode). A held
    /// descriptor keeps its inode allocated, so the key cannot name another
    /// file while the entry exists, and entries are never removed.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    held: BTreeMap<(u64, u64), std::fs::File>,
    /// Every marked incarnation id this process minted.
    minted: BTreeSet<String>,
}

fn process_custody() -> std::sync::MutexGuard<'static, ProcessCustody> {
    static CUSTODY: OnceLock<Mutex<ProcessCustody>> = OnceLock::new();
    CUSTODY
        .get_or_init(|| Mutex::new(ProcessCustody::default()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Mint the incarnation id of a new actor build over the identity store
/// file `store` (`None` for a store without a file). The id is marked only
/// when this process holds, or now takes, the store's incarnation custody.
pub(crate) fn mint_incarnation_id(store: Option<&Path>) -> String {
    let id = meerkat_core::ops::OperationId::new().to_string();
    let Some(store) = store else {
        return id;
    };
    let mut custody = process_custody();
    if !hold_store_custody(&mut custody, store) {
        return id;
    }
    let marked = format!("{CUSTODY_MARK}{id}");
    custody.minted.insert(marked.clone());
    marked
}

/// Whether the process of the incarnation `other`, which holds a lease of
/// the store that the incarnation `own` was minted for, has ended.
///
/// True only when `own` is marked and minted here (so this process holds
/// the store's custody) and `other` is marked and was not minted here.
pub(crate) fn incarnation_process_ended(own: &str, other: &str) -> bool {
    if !own.starts_with(CUSTODY_MARK) || !other.starts_with(CUSTODY_MARK) || own == other {
        return false;
    }
    let custody = process_custody();
    custody.minted.contains(own) && !custody.minted.contains(other)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn hold_store_custody(custody: &mut ProcessCustody, store: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    // A store with more than one link has one lock file per directory it is
    // linked from.
    match std::fs::metadata(store) {
        Ok(metadata) if metadata.is_file() && metadata.nlink() == 1 => {}
        _ => return false,
    }
    let mut lock_path = store.as_os_str().to_os_string();
    lock_path.push(LOCK_SUFFIX);
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
    else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if !metadata.is_file() || metadata.nlink() != 1 {
        return false;
    }
    let key = (metadata.dev(), metadata.ino());
    if custody.held.contains_key(&key) {
        return true;
    }
    // A second descriptor of this process would conflict with the held one,
    // so the held map is consulted first; a conflict here is another process.
    if file.try_lock().is_err() {
        return false;
    }
    custody.held.insert(key, file);
    true
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn hold_store_custody(_custody: &mut ProcessCustody, _store: &Path) -> bool {
    false
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;

    fn store_file(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let store = dir.path().join("mob.db");
        std::fs::write(&store, b"").expect("store file");
        store
    }

    #[test]
    fn an_incarnation_minted_under_custody_is_marked_and_never_ended_here() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_file(&dir);
        let first = mint_incarnation_id(Some(&store));
        let second = mint_incarnation_id(Some(&store));
        assert!(first.starts_with(CUSTODY_MARK), "{first}");
        assert!(second.starts_with(CUSTODY_MARK), "{second}");
        assert!(
            !incarnation_process_ended(&second, &first),
            "an earlier incarnation of this process may still run"
        );
        let foreign = format!("{CUSTODY_MARK}{}", meerkat_core::ops::OperationId::new());
        assert!(incarnation_process_ended(&second, &foreign));
        assert!(!incarnation_process_ended(&second, "unmarked-incarnation"));
        assert!(!incarnation_process_ended(&second, &second));
    }

    #[test]
    fn a_store_without_a_file_mints_unmarked_incarnations() {
        let id = mint_incarnation_id(None);
        assert!(!id.starts_with(CUSTODY_MARK), "{id}");
        let foreign = format!("{CUSTODY_MARK}{}", meerkat_core::ops::OperationId::new());
        assert!(!incarnation_process_ended(&id, &foreign));
    }

    #[test]
    fn custody_held_by_another_descriptor_mints_unmarked_incarnations() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_file(&dir);
        let mut lock_path = store.as_os_str().to_os_string();
        lock_path.push(LOCK_SUFFIX);
        let other_process = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .expect("lock file");
        other_process
            .try_lock()
            .expect("the other holder takes custody");
        let id = mint_incarnation_id(Some(&store));
        assert!(!id.starts_with(CUSTODY_MARK), "{id}");
        let foreign = format!("{CUSTODY_MARK}{}", meerkat_core::ops::OperationId::new());
        assert!(!incarnation_process_ended(&id, &foreign));
    }

    /// The store's sweeps (doctor inventory, prune guard) name the lock file
    /// by the same suffix the custody takes.
    #[test]
    fn the_store_sweeps_name_this_lock_file() {
        assert_eq!(
            LOCK_SUFFIX,
            meerkat_store::migrate::MOB_INCARNATION_CUSTODY_LOCK_SUFFIX
        );
    }

    #[test]
    fn a_store_with_a_second_link_mints_unmarked_incarnations() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_file(&dir);
        std::fs::hard_link(&store, dir.path().join("alias.db")).expect("second link");
        let id = mint_incarnation_id(Some(&store));
        assert!(!id.starts_with(CUSTODY_MARK), "{id}");
    }
}

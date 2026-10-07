use super::*;
use crate::store::execution_custody::ExecutionCustodyOwner;
use crate::store::{RuntimeStore, RuntimeStoreExecutionCustody, SqliteRuntimeStore};

fn actual_custody(store: &SqliteRuntimeStore) -> &RuntimeStoreExecutionCustody {
    RuntimeStore::execution_custody(store)
        .expect("the actual SQLite backend owns execution custody")
}

fn physical_owner(store: &SqliteRuntimeStore) -> Arc<PhysicalExecutionCustody> {
    match &actual_custody(store).inner {
        ExecutionCustodyOwner::Physical(owner) => Arc::clone(owner),
        ExecutionCustodyOwner::Memory(_) => {
            panic!("SQLite cannot delegate physical claims to memory");
        }
    }
}

fn assert_admission_busy(store: &SqliteRuntimeStore) {
    assert!(matches!(
        actual_custody(store).try_acquire_shared(),
        Err(CustodyError::Busy)
    ));
    assert!(matches!(
        actual_custody(store).try_acquire_governed(),
        Err(CustodyError::Busy)
    ));
}

#[test]
fn sqlite_open_does_not_create_execution_locks_before_a_claim() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new_whole_blob(path.clone()).unwrap();
    let peer = SqliteRuntimeStore::new_whole_blob(path).unwrap();
    let owner = physical_owner(&store);
    assert!(!owner.lifetime_path.exists());
    assert!(!owner.admission_path.exists());
    drop(peer);
    drop(store);
    assert!(!owner.lifetime_path.exists());
    assert!(!owner.admission_path.exists());
}

#[test]
fn sqlite_memory_database_cannot_mint_physical_file_custody() {
    assert!(matches!(
        SqliteRuntimeStore::new_whole_blob(":memory:"),
        Err(crate::store::RuntimeStoreError::Unsupported(_))
    ));
    assert!(matches!(
        SqliteRuntimeStore::new_head_canonical(":memory:"),
        Err(crate::store::RuntimeStoreError::Unsupported(_))
    ));
}

#[test]
fn sqlite_upgrade_unlock_failure_retains_physical_gate_until_last_claim() {
    for fault in [UpgradeFault::Unlock, UpgradeFault::UnlockAfterRelease] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runtime.sqlite3");
        let store = SqliteRuntimeStore::new_whole_blob(path.clone()).unwrap();
        let peer = SqliteRuntimeStore::new_whole_blob(path).unwrap();
        let owner = physical_owner(&store);
        let mut claim = actual_custody(&store).try_acquire_shared().unwrap();
        owner
            .upgrade_fault
            .store(fault as u8, std::sync::atomic::Ordering::SeqCst);

        assert_eq!(
            claim.try_upgrade_to_governed(),
            Err(CustodyError::Unavailable)
        );
        assert!(!claim.is_governed());
        assert_eq!(
            claim.try_upgrade_to_governed(),
            Err(CustodyError::Unavailable)
        );
        let raw_lifetime = open_lock_file(&owner.lifetime_path).unwrap();
        if fault == UpgradeFault::Unlock {
            assert!(matches!(
                raw_lifetime.try_lock(),
                Err(TryLockError::WouldBlock)
            ));
        } else {
            raw_lifetime.try_lock().unwrap();
        }
        assert_admission_busy(&peer);
        drop(raw_lifetime);

        let claim = Arc::new(claim);
        let retained = Arc::clone(&claim);
        drop(store);
        drop(claim);
        assert_admission_busy(&peer);
        drop(retained);
        actual_custody(&peer).try_acquire_governed().unwrap();
        actual_custody(&peer).try_acquire_shared().unwrap();
    }
}

#[test]
fn sqlite_upgrade_restoration_failure_blocks_admission_after_shared_lock_loss() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new_whole_blob(path.clone()).unwrap();
    let peer = SqliteRuntimeStore::new_whole_blob(path).unwrap();
    let owner = physical_owner(&store);
    let mut claim = actual_custody(&store).try_acquire_shared().unwrap();
    let competing = actual_custody(&peer).try_acquire_shared().unwrap();
    owner.upgrade_fault.store(
        UpgradeFault::Restore as u8,
        std::sync::atomic::Ordering::SeqCst,
    );

    assert_eq!(
        claim.try_upgrade_to_governed(),
        Err(CustodyError::Unavailable)
    );
    assert!(!claim.is_governed());
    assert_eq!(
        claim.try_upgrade_to_governed(),
        Err(CustodyError::Unavailable)
    );
    drop(competing);
    // The real lifetime descriptor is now unlocked. Only the failed claim's
    // separately retained physical admission gate prevents a second owner.
    let raw_lifetime = open_lock_file(&owner.lifetime_path).unwrap();
    raw_lifetime.try_lock().unwrap();
    assert_admission_busy(&peer);
    drop(raw_lifetime);

    let claim = Arc::new(claim);
    let retained = Arc::clone(&claim);
    drop(store);
    drop(claim);
    assert_admission_busy(&peer);
    drop(retained);
    actual_custody(&peer).try_acquire_governed().unwrap();
    actual_custody(&peer).try_acquire_shared().unwrap();
}

#[cfg(unix)]
#[test]
fn sqlite_store_and_execution_owner_bind_the_same_canonical_database() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.sqlite3");
    let original = SqliteRuntimeStore::new_whole_blob(path.clone()).unwrap();
    let alias = directory.path().join("alias.sqlite3");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    let alias = SqliteRuntimeStore::new_whole_blob(alias).unwrap();
    let physical = std::fs::canonicalize(path).unwrap();
    assert_eq!(original.path(), physical);
    assert_eq!(alias.path(), physical);
    assert_eq!(
        physical_owner(&original).lifetime_path,
        physical_owner(&alias).lifetime_path
    );
    assert_eq!(
        physical_owner(&original).admission_path,
        physical_owner(&alias).admission_path
    );
}

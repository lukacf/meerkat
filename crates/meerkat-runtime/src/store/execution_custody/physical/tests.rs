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
        // Released, not only closed: the peer takes custody at once below.
        let _ = raw_lifetime.unlock();
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
    raw_lifetime.unlock().unwrap();
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

/// A governed claim retains another database's governed claim for exactly
/// its own lifetime: that database's owners stay out while it lives, the
/// association passes neither store's mutation authorization to the other
/// store's owner, and both are free once it is dropped.
#[test]
fn an_associated_claim_is_held_with_its_claim_and_authorizes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let host = SqliteRuntimeStore::new_whole_blob(directory.path().join("host.db")).unwrap();
    let realm = SqliteRuntimeStore::new_whole_blob(directory.path().join("realm.sqlite3")).unwrap();
    let associated = SqliteRuntimeStore::try_acquire_governed_execution_custody(
        directory.path().join("realm.sqlite3"),
    )
    .unwrap();
    assert!(!actual_custody(&realm).owns_governed_claim(&associated));
    let mut claim = actual_custody(&host).try_acquire_governed().unwrap();
    claim.retain_associated(associated).unwrap();

    assert_admission_busy(&realm);
    assert_admission_busy(&host);
    assert!(actual_custody(&host).owns_governed_claim(&claim));
    assert!(
        !actual_custody(&realm).owns_governed_claim(&claim),
        "the association is no authority over the associated store"
    );
    assert!(!actual_custody(&realm).binds_same_store(&claim));

    drop(claim);
    drop(actual_custody(&realm).try_acquire_governed().unwrap());
    drop(actual_custody(&host).try_acquire_governed().unwrap());
}

/// A claim taken through another path to the same database binds that
/// store; a machine over the store refuses it as its associated claim and
/// releases it, so the store's own custody is free again.
#[cfg(unix)]
#[test]
fn a_claim_on_the_same_database_by_another_path_is_never_associated() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.sqlite3");
    let store = Arc::new(SqliteRuntimeStore::new_whole_blob(path.clone()).unwrap());
    let alias = directory.path().join("alias.sqlite3");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    let aliased = SqliteRuntimeStore::try_acquire_governed_execution_custody(alias).unwrap();
    assert!(actual_custody(&store).binds_same_store(&aliased));
    assert!(
        crate::MeerkatMachine::persistent_governed_staged_with_associated_custody(
            Arc::clone(&store) as Arc<dyn RuntimeStore>,
            None,
            aliased,
        )
        .is_err(),
        "the machine never admits itself through an alias"
    );
    drop(actual_custody(&store).try_acquire_governed().unwrap());
}

/// A missing database has no custody to take.
#[test]
fn a_missing_database_has_no_custody_to_take() {
    let directory = tempfile::tempdir().unwrap();
    assert!(matches!(
        SqliteRuntimeStore::try_acquire_governed_execution_custody(
            directory.path().join("absent.sqlite3")
        ),
        Err(CustodyError::Unavailable)
    ));
}

/// A process this test spawns that holds a copy of one of the test's
/// descriptors, as its standard input, until it is dropped: what any process
/// spawned meanwhile holds until it executes its program, kept for as long as
/// the test needs.
#[cfg(unix)]
struct DescriptorCopy(std::process::Child);

#[cfg(unix)]
impl DescriptorCopy {
    /// Spawn a process holding a copy of `file`'s descriptor.
    fn spawn(file: &std::fs::File) -> Self {
        let copy = file.try_clone().expect("a copy of the descriptor");
        #[allow(clippy::disallowed_methods)] // the test's own process, which holds the copy
        let child = std::process::Command::new("sleep")
            .arg("600")
            .stdin(copy)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("a process holding the copy");
        Self(child)
    }
}

#[cfg(unix)]
impl Drop for DescriptorCopy {
    fn drop(&mut self) {
        // On request (MEERKAT_TEST_DESCRIPTOR_COPY_LINGER_MS), the copy stays open
        // that long first, so an fd sampler watching the run sees it.
        if let Some(linger) = std::env::var("MEERKAT_TEST_DESCRIPTOR_COPY_LINGER_MS")
            .ok()
            .and_then(|millis| millis.parse::<u64>().ok())
        {
            std::thread::sleep(std::time::Duration::from_millis(linger));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A released execution claim is free at once for the next owner while a
/// process spawned meanwhile still holds a copy of its descriptor.
#[cfg(unix)]
#[test]
fn a_released_claim_is_free_while_a_spawned_process_holds_its_descriptor() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new_whole_blob(path.clone()).unwrap();
    let peer = SqliteRuntimeStore::new_whole_blob(path).unwrap();
    let claim = physical_owner(&store).try_acquire(true).unwrap();
    let copy = DescriptorCopy::spawn(&claim.lifetime);
    drop(claim);
    assert!(
        physical_owner(&peer).try_acquire(true).is_ok(),
        "a released claim is free while a spawned process holds a copy of its descriptor"
    );
    drop(copy);
}

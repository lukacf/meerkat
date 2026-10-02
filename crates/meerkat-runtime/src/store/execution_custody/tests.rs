use super::*;

#[test]
fn failed_upgrade_keeps_the_same_shared_claim() {
    let owner = RuntimeStoreExecutionCustody::new();
    let mut first = owner.try_acquire_shared().unwrap();
    let second = owner.try_acquire_shared().unwrap();
    assert_eq!(
        first.try_upgrade_to_governed(),
        Err(RuntimeStoreExecutionCustodyError::Busy)
    );
    assert!(!first.is_governed());
    let third = owner
        .try_acquire_shared()
        .expect("failed upgrade did not publish exclusive state");
    drop(third);
    drop(second);
    first
        .try_upgrade_to_governed()
        .expect("one atomic upgrade after actual quiescence");
    assert!(first.is_governed());
    assert!(matches!(
        owner.try_acquire_shared(),
        Err(RuntimeStoreExecutionCustodyError::Busy)
    ));
    drop(first);
    owner
        .try_acquire_shared()
        .expect("owner is reusable after final guard drop");
}

#[test]
fn retained_guard_clone_does_not_release_the_scope_early() {
    let owner = RuntimeStoreExecutionCustody::new();
    let guard = owner.try_acquire_governed().unwrap();
    let guard = Arc::new(guard);
    let retained = Arc::clone(&guard);
    drop(guard);
    assert!(matches!(
        owner.try_acquire_shared(),
        Err(RuntimeStoreExecutionCustodyError::Busy)
    ));
    drop(retained);
    owner.try_acquire_shared().unwrap();
}

#[test]
fn upfront_governed_claim_requires_all_shared_owners_to_release() {
    let owner = RuntimeStoreExecutionCustody::new();
    let same_owner = owner.clone();
    let first = owner.try_acquire_shared().unwrap();
    let second = same_owner.try_acquire_shared().unwrap();
    assert!(matches!(
        owner.try_acquire_governed(),
        Err(RuntimeStoreExecutionCustodyError::Busy)
    ));
    drop(first);
    assert!(matches!(
        owner.try_acquire_governed(),
        Err(RuntimeStoreExecutionCustodyError::Busy)
    ));
    drop(second);
    let governed = same_owner.try_acquire_governed().unwrap();
    assert!(governed.is_governed());
    assert!(matches!(
        owner.try_acquire_shared(),
        Err(RuntimeStoreExecutionCustodyError::Busy)
    ));
    assert!(matches!(
        owner.try_acquire_governed(),
        Err(RuntimeStoreExecutionCustodyError::Busy)
    ));
    drop(governed);
    owner.try_acquire_governed().unwrap();
}

#[test]
fn poisoned_custody_owner_never_reopens_execution() {
    let owner = RuntimeStoreExecutionCustody::new();
    let _ = std::panic::catch_unwind(|| {
        let _actual_owner = owner.inner.lock().unwrap();
        panic!("fault inside actual custody owner");
    });
    assert!(matches!(
        owner.try_acquire_shared(),
        Err(RuntimeStoreExecutionCustodyError::Unavailable)
    ));
    assert!(matches!(
        owner.try_acquire_governed(),
        Err(RuntimeStoreExecutionCustodyError::Unavailable)
    ));
}

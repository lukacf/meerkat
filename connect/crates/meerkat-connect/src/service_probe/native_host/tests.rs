//! Mechanical native owner controls, not parent authentication or provider proof.
use super::*;
use crate::service_probe::{LocalAuthorizationPublication, now, tests::binding};
use std::sync::atomic::Ordering;

fn host(root: &std::path::Path) -> Arc<ServiceHost> {
    ServiceHost::new(binding(root)).unwrap()
}

#[tokio::test]
async fn native_fence_invalidates_actual_publication_without_claiming_close() {
    let root = tempfile::tempdir().unwrap();
    let host = host(root.path());
    let other = ServiceHost::new(binding(root.path())).unwrap();
    let publication = LocalAuthorizationPublication::new();
    host.lifetime.join(publication.clone()).unwrap();
    let (_, before) = publication.observe(|| ()).unwrap();
    let control = host.control();
    let fence = control.fence().unwrap();
    assert!(fence.belongs_to(&control));
    assert!(!fence.belongs_to(&other.control()));
    assert!(before.check_current().is_err());
    assert!(!host.lifetime.current(now() + 60_000));
    assert!(
        host.lifetime
            .join(LocalAuthorizationPublication::new())
            .is_err()
    );
    // Nothing entered the actual stdio owner. Report that without waiting for
    // work that can no longer register, and without inventing a joined close.
    assert!(fence.joined().await.is_err());
    assert!(NativeHostInvocation::enter(host).is_err());
    assert!(control.fence().unwrap().belongs_to(&control));
}

#[tokio::test]
async fn native_retirement_before_registration_refuses_invocation_without_waiting() {
    let root = tempfile::tempdir().unwrap();
    let host = host(root.path());
    let control = host.control();
    control.retire();
    assert!(NativeHostInvocation::enter(host).is_err());
    assert!(control.fence().unwrap().joined().await.is_err());
}

#[test]
fn poisoned_native_publication_retires_but_cannot_issue_fence_acknowledgement() {
    let root = tempfile::tempdir().unwrap();
    let host = host(root.path());
    let publication = LocalAuthorizationPublication::new();
    host.lifetime.join(publication.clone()).unwrap();
    publication.poison();
    assert!(host.control().fence().is_err());
    assert!(host.lifetime.retired.load(Ordering::Acquire));
    assert!(!host.lifetime.current(now() + 60_000));
    assert!(
        host.control().fence().is_err(),
        "a later observer cannot repair the poisoned owner"
    );
}

#[test]
fn poisoned_publication_registry_does_not_acknowledge_native_fence() {
    let root = tempfile::tempdir().unwrap();
    let host = host(root.path());
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _owned = host.lifetime.publications.lock().unwrap();
        panic!("publication registration owner failed");
    }));
    assert!(panicked.is_err());
    assert!(host.control().fence().is_err());
    assert!(!host.lifetime.current(now() + 60_000));
}

#[tokio::test]
async fn lost_native_invocation_retains_unconfirmed_close_and_refuses_reentry() {
    let root = tempfile::tempdir().unwrap();
    let host = host(root.path());
    let invocation = NativeHostInvocation::enter(host.clone()).unwrap();
    assert!(NativeHostInvocation::enter(host.clone()).is_err());
    let control = host.control();
    let fence = control.fence().unwrap();
    let joined = fence.joined();
    tokio::pin!(joined);
    assert!(futures_util::poll!(joined.as_mut()).is_pending());
    drop(invocation);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), joined)
            .await
            .unwrap()
            .is_err()
    );
    assert!(NativeHostInvocation::enter(host).is_err());
    assert!(control.fence().unwrap().joined().await.is_err());
}

#[test]
fn poisoned_invocation_custody_still_retires_actual_native_publication_without_acknowledgement() {
    let root = tempfile::tempdir().unwrap();
    let host = host(root.path());
    let publication = LocalAuthorizationPublication::new();
    host.lifetime.join(publication.clone()).unwrap();
    let (_, before) = publication.observe(|| ()).unwrap();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _owned = host.lifetime.invocation.state.lock().unwrap();
        panic!("invocation custody owner failed");
    }));
    assert!(panicked.is_err());
    assert!(!host.lifetime.retired.load(Ordering::Acquire));
    assert!(host.control().fence().is_err());
    assert!(host.lifetime.retired.load(Ordering::Acquire));
    assert!(before.check_current().is_err());
    assert!(NativeHostInvocation::enter(host).is_err());
}

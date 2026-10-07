//! Exact paired setter semantics; no operation permission is fabricated here.
use super::*;
use crate::handles::{RuntimeAuthLeaseHandle, RuntimeOAuthFlowHandle};
use crate::traits::ControllerReadinessFailure;
use meerkat_auth_core::oauth_flow::OAuthFlowAuthority;
use std::time::Duration;

fn oauth(handle: &Arc<RuntimeAuthLeaseHandle>) -> Arc<dyn OAuthFlowAuthority> {
    Arc::new(RuntimeOAuthFlowHandle::new_with_auth_lease(
        Duration::from_secs(60),
        handle.clone(),
    ))
}

fn assert_pair(
    machine: &MeerkatMachine,
    expected_auth: &Arc<RuntimeAuthLeaseHandle>,
    expected_oauth: &Arc<dyn OAuthFlowAuthority>,
) {
    let pair = machine.provider_auth_runtime_authority();
    let auth: Arc<dyn meerkat_core::handles::AuthLeaseHandle> = expected_auth.clone();
    assert!(Arc::ptr_eq(
        &pair.generated_auth_lease_handle().clone_handle(),
        &auth,
    ));
    assert!(Arc::ptr_eq(&pair.oauth_flow_authority(), expected_oauth));
}

fn governed_machine() -> (MeerkatMachine, Arc<RuntimeAuthLeaseHandle>) {
    let machine = MeerkatMachine::ephemeral();
    let auth = Arc::new(RuntimeAuthLeaseHandle::new());
    machine.set_runtime_auth_lease_handle(auth.clone()).unwrap();
    let host = Arc::new(crate::input_authority::tests::TestIngress::new(
        machine.generated_auth_lease_handle(),
    ));
    let machine = machine.with_native_work_authorization_host(host).unwrap();
    assert!(machine.credential_release_observer.get().is_some());
    (machine, auth)
}

#[test]
fn same_auth_handle_rejects_changed_oauth_authority_without_mutation() {
    let (machine, auth) = governed_machine();
    let original = machine
        .provider_auth_runtime_authority()
        .oauth_flow_authority();

    // The normal setter keeps its existing successful same-handle no-op.
    machine.set_runtime_auth_lease_handle(auth.clone()).unwrap();
    assert_pair(&machine, &auth, &original);
    machine
        .set_auth_lease_handle_with_oauth_flow_authority(auth.clone(), original.clone())
        .unwrap();
    assert_pair(&machine, &auth, &original);

    let requested = oauth(&auth);
    assert!(!Arc::ptr_eq(&original, &requested));
    let result =
        machine.set_auth_lease_handle_with_oauth_flow_authority(auth.clone(), requested.clone());
    assert!(
        matches!(
            result,
            Err(RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::UnsupportedScope,
            })
        ),
        "a changed OAuth half cannot silently succeed as an unchanged pair"
    );
    assert_pair(&machine, &auth, &original);
    assert!(!Arc::ptr_eq(&machine.oauth_flow_authority(), &requested));
    machine.set_runtime_auth_lease_handle(auth.clone()).unwrap();
    assert_pair(&machine, &auth, &original);
}

#[test]
fn different_auth_handle_installs_requested_pair() {
    let (machine, old_auth) = governed_machine();
    let old_oauth = machine.oauth_flow_authority();
    let replacement = Arc::new(RuntimeAuthLeaseHandle::new());
    let requested = oauth(&replacement);
    machine
        .set_auth_lease_handle_with_oauth_flow_authority(replacement.clone(), requested.clone())
        .unwrap();
    assert_pair(&machine, &replacement, &requested);
    let old: Arc<dyn meerkat_core::handles::AuthLeaseHandle> = old_auth;
    assert!(!Arc::ptr_eq(&machine.auth_lease_handle(), &old));
    assert!(!Arc::ptr_eq(&machine.oauth_flow_authority(), &old_oauth));
}

#[test]
fn unconfigured_same_auth_handle_can_install_explicit_oauth_authority() {
    let machine = MeerkatMachine::ephemeral();
    let auth = Arc::new(RuntimeAuthLeaseHandle::new());
    machine.set_runtime_auth_lease_handle(auth.clone()).unwrap();
    assert!(machine.credential_release_observer.get().is_none());
    let original = machine.oauth_flow_authority();
    let requested = oauth(&auth);
    assert!(!Arc::ptr_eq(&original, &requested));
    machine
        .set_auth_lease_handle_with_oauth_flow_authority(auth.clone(), requested.clone())
        .unwrap();
    assert_pair(&machine, &auth, &requested);
}

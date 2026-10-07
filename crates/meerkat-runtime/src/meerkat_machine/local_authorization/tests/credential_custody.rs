//! Real credential owner and native accepted-row custody controls.
//! API-dependent source tests. No execution is claimed by this packet.
use super::*;
use crate::traits::{ControllerReadinessFailure, RuntimeDriverError};
use meerkat_auth_core::{EphemeralTokenStore, InMemoryCoordinator};
use meerkat_core::auth::{PersistedTokens, ProviderAuthPersistence, TokenStore};
use meerkat_core::handles::{CredentialUseDisposition, LeaseKey};

async fn pending() -> (MeerkatMachine, SessionId, Input) {
    let (configuration, mut prompt, _, _) = mutable_controller_configuration(true);
    let mut candidate = prompt
        .header()
        .authority_association
        .as_ref()
        .unwrap()
        .candidate()
        .clone();
    let selection = candidate.controller_model.as_ref().unwrap();
    let credential =
        meerkat_core::AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
            realm: meerkat_core::RealmId::parse("native-test").unwrap(),
            account: meerkat_core::CredentialAccountId::parse(format!(
                "custody-{}",
                uuid::Uuid::new_v4()
            ))
            .unwrap(),
        });
    candidate.controller_model = Some(ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: selection.model().into(),
            provider: selection.provider(),
            self_hosted_server_id: selection.self_hosted_server_id().map(str::to_owned),
            provider_params: None,
            auth_binding: selection.auth_binding().cloned(),
        },
        credential,
        selection.backend_profile_id().into(),
        selection.backend_kind().into(),
    ));
    prompt.header_mut().authority_association =
        Some(InputAuthorityAssociation::new(candidate).unwrap());
    pending_controller_input(configuration, prompt).await
}

fn identity(prompt: &Input) -> meerkat_core::AuthCredentialIdentity {
    prompt
        .header()
        .ingress_context
        .as_ref()
        .unwrap()
        .controller_client()
        .unwrap()
        .selection()
        .credential()
        .clone()
}

async fn commit(
    machine: &MeerkatMachine,
    prompt: &Input,
) -> (
    Arc<EphemeralTokenStore>,
    meerkat_core::handles::GeneratedAuthLeaseHandle,
) {
    let store = Arc::new(EphemeralTokenStore::new());
    let handle = machine.generated_auth_lease_handle();
    meerkat_auth_core::save_tokens_and_publish_lifecycle(
        ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
        handle.clone(),
        identity(prompt),
        PersistedTokens::api_key("synthetic-custody-test"),
    )
    .await
    .unwrap();
    (store, handle)
}

async fn attempt(
    machine: &MeerkatMachine,
    session: &SessionId,
    prompt: Input,
) -> Result<crate::accept::AcceptOutcome, RuntimeDriverError> {
    let driver = machine
        .sessions
        .read()
        .await
        .get(session)
        .unwrap()
        .driver
        .clone();
    let mut locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &mut *locked else {
        panic!("storeless")
    };
    driver.set_executor_work_authorization_support(true);
    driver.accept_input(prompt).await
}

async fn assert_unaccepted(
    machine: &MeerkatMachine,
    session: &SessionId,
    id: &meerkat_core::InputId,
) {
    let driver = machine
        .sessions
        .read()
        .await
        .get(session)
        .unwrap()
        .driver
        .clone();
    let locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &*locked else {
        panic!("storeless")
    };
    assert!(driver.ledger().get(id).is_none());
}

fn assert_release_veto(
    handle: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    key: &LeaseKey,
    expected_context: &'static str,
) {
    assert_eq!(
        handle
            .resolve_credential_use_admission(
                key,
                meerkat_core::handles::CredentialUseIntent::HoldAuthority
            )
            .unwrap(),
        CredentialUseDisposition::Authorized
    );
    let error = handle
        .release_lease(key)
        .expect_err("native observer must veto before AuthMachine Release");
    assert_eq!(
        error.kind,
        meerkat_core::handles::DslRejectionKind::NoMatchingTransition
    );
    assert_eq!(error.context, expected_context);
}

#[tokio::test]
async fn direct_driver_busy_lease_is_typed_readiness_then_same_input_can_enter() {
    let (machine, session, prompt) = pending().await;
    let (_store, _handle) = commit(&machine, &prompt).await;
    let key = LeaseKey::from_credential_identity(&identity(&prompt));
    let held = meerkat_core::acquire_auth_login_lifecycle_guard(&key).await;
    let id = prompt.id().clone();
    let before = machine.session_dsl_state(&session).await.unwrap();
    assert!(matches!(
        attempt(&machine, &session, prompt.clone()).await,
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::Busy
        })
    ));
    assert_unaccepted(&machine, &session, &id).await;
    assert_eq!(machine.session_dsl_state(&session).await.unwrap(), before);
    drop(held);
    assert!(
        matches!(attempt(&machine, &session, prompt).await.unwrap(), crate::accept::AcceptOutcome::Accepted { input_id, .. } if input_id == id)
    );
}

#[tokio::test]
async fn absent_credential_is_not_permission_feedback_and_commit_allows_same_input() {
    let (machine, session, prompt) = pending().await;
    let id = prompt.id().clone();
    assert!(matches!(
        attempt(&machine, &session, prompt.clone()).await,
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::CredentialUnusable {
                disposition: CredentialUseDisposition::LeaseAbsent
            }
        })
    ));
    assert_unaccepted(&machine, &session, &id).await;
    let (_store, _handle) = commit(&machine, &prompt).await;
    attempt(&machine, &session, prompt).await.unwrap();
}

#[tokio::test]
async fn exported_handle_release_retains_consumed_controller_until_real_run_terminal() {
    let (machine, session, prompt) = pending().await;
    let credential = identity(&prompt);
    let key = LeaseKey::from_credential_identity(&credential);
    let (store, handle) = commit(&machine, &prompt).await;
    let before = handle.snapshot(&key);
    let stored = store
        .load(&meerkat_core::auth::TokenKey::from_credential_identity(
            &credential,
        ))
        .await
        .unwrap();
    let (id, run, _) = stage_controller_input(&machine, &session, prompt).await;
    let driver = machine
        .sessions
        .read()
        .await
        .get(&session)
        .unwrap()
        .driver
        .clone();
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver
            .abandon_all_non_terminal(crate::input_state::InputAbandonReason::Stopped)
            .unwrap();
        assert!(driver.input_is_terminal_by_authority(&id).unwrap());
    }
    assert_release_veto(&handle, &key, "NativeControllerCredentialRelease::InUse");
    assert_eq!(handle.snapshot(&key), before);
    assert_eq!(
        store
            .load(&meerkat_core::auth::TokenKey::from_credential_identity(
                &credential
            ))
            .await
            .unwrap(),
        stored
    );
    {
        let locked = driver.lock().await;
        let authority = locked.shared_dsl_authority();
        let mut owner = authority.lock().unwrap();
        dsl::MeerkatMachineMutator::apply(
            &mut *owner,
            dsl::MeerkatMachineInput::RunCompleted {
                run_id: dsl::RunId::from_domain(&run),
            },
        )
        .unwrap();
    }
    handle.release_lease(&key).unwrap();
    assert!(!handle.snapshot(&key).credential_present);
}

#[tokio::test]
async fn active_controller_refuses_handle_replacement_without_detaching_old_observer() {
    let (machine, session, prompt) = pending().await;
    let key = LeaseKey::from_credential_identity(&identity(&prompt));
    let (_store, old) = commit(&machine, &prompt).await;
    let (_, run, context) = stage_controller_input(&machine, &session, prompt).await;
    let replacement = Arc::new(crate::handles::RuntimeAuthLeaseHandle::new());
    assert!(matches!(
        machine.set_runtime_auth_lease_handle(replacement),
        Err(RuntimeDriverError::ControllerInUse)
    ));
    assert!(Arc::ptr_eq(
        &old.clone_handle(),
        &machine.generated_auth_lease_handle().clone_handle()
    ));
    assert_release_veto(&old, &key, "NativeControllerCredentialRelease::InUse");
    meerkat_core::authorization::PreparedOperationCheck::prepare(
        context.clone(),
        plain_controller_binding(&context, &run),
    )
    .unwrap()
    .current()
    .unwrap();
}

#[tokio::test]
async fn replacement_and_previously_exported_handles_both_keep_native_veto() {
    let (machine, session, prompt) = pending().await;
    let key = LeaseKey::from_credential_identity(&identity(&prompt));
    let (_old_store, old) = commit(&machine, &prompt).await;
    machine
        .set_runtime_auth_lease_handle(Arc::new(crate::handles::RuntimeAuthLeaseHandle::new()))
        .unwrap();
    let (_new_store, current) = commit(&machine, &prompt).await;
    assert!(!Arc::ptr_eq(&old.clone_handle(), &current.clone_handle()));
    stage_controller_input(&machine, &session, prompt).await;
    for handle in [old, current] {
        let before = handle.snapshot(&key);
        assert_release_veto(&handle, &key, "NativeControllerCredentialRelease::InUse");
        assert_eq!(handle.snapshot(&key), before);
    }
}

#[tokio::test]
async fn unrelated_credential_release_is_allowed_and_busy_native_scope_is_not_mutated() {
    let (machine, session, prompt) = pending().await;
    let key = LeaseKey::from_credential_identity(&identity(&prompt));
    let (_store, handle) = commit(&machine, &prompt).await;
    stage_controller_input(&machine, &session, prompt).await;
    let unrelated =
        meerkat_core::AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
            realm: meerkat_core::RealmId::parse("native-test").unwrap(),
            account: meerkat_core::CredentialAccountId::parse(format!(
                "unrelated-{}",
                uuid::Uuid::new_v4()
            ))
            .unwrap(),
        });
    let other_key = LeaseKey::from_credential_identity(&unrelated);
    meerkat_core::publish_token_lifecycle_acquired_for_identity(
        &handle,
        &unrelated,
        &PersistedTokens::api_key("synthetic-other"),
    )
    .unwrap();
    handle.release_lease(&other_key).unwrap();
    let before = handle.snapshot(&key);
    let driver = machine
        .sessions
        .read()
        .await
        .get(&session)
        .unwrap()
        .driver
        .clone();
    let held = driver.lock().await;
    assert_release_veto(&handle, &key, "NativeControllerCredentialRelease::Busy");
    assert_eq!(handle.snapshot(&key), before);
    drop(held);
}

#[tokio::test]
async fn authority_replacement_while_waiting_requires_fresh_admission() {
    let (machine, session, prompt) = pending().await;
    let (_store, _handle) = commit(&machine, &prompt).await;
    let key = LeaseKey::from_credential_identity(&identity(&prompt));
    let held = meerkat_core::acquire_auth_login_lifecycle_guard(&key).await;
    let mut acquisition = Box::pin(
        crate::meerkat_machine::credential_custody::NativeCredentialCustody::acquire(
            &machine.native_work_authorization_host,
            &prompt,
            false,
        ),
    );
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(
            std::future::Future::poll(acquisition.as_mut(), cx).is_pending()
        ))
        .await,
        "actual acquisition captured old owner and reached the held lease"
    );
    machine
        .set_runtime_auth_lease_handle(Arc::new(crate::handles::RuntimeAuthLeaseHandle::new()))
        .unwrap();
    drop(held);
    let custody = acquisition.await.unwrap();
    assert!(matches!(
        custody.validate(&machine.native_work_authorization_host, &prompt),
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::AuthorityChanged
        })
    ));
    assert_unaccepted(&machine, &session, prompt.id()).await;
}

#[test]
fn native_control_conversion_preserves_each_readiness_cause() {
    for reason in [
        ControllerReadinessFailure::Busy,
        ControllerReadinessFailure::AuthorityUnavailable,
        ControllerReadinessFailure::AuthorityChanged,
        ControllerReadinessFailure::UnsupportedScope,
        ControllerReadinessFailure::PolicyChanged,
        ControllerReadinessFailure::CredentialUnusable {
            disposition: CredentialUseDisposition::LeaseAbsent,
        },
    ] {
        let control = MeerkatMachine::control_plane_error_from_driver_error(
            RuntimeDriverError::ControllerReadinessUnavailable { reason },
        );
        assert!(
            matches!(control, crate::traits::RuntimeControlPlaneError::ControllerReadinessUnavailable { reason: actual } if actual == reason)
        );
        let driver = MeerkatMachine::driver_error_from_control_plane_error(control);
        assert!(
            matches!(driver, RuntimeDriverError::ControllerReadinessUnavailable { reason: actual } if actual == reason)
        );
    }
}

struct ReleaseGate(Arc<crate::handles::FailingOAuthSnapshotStore>);
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        self.0.release_blocked_oauth_persist();
    }
}

async fn oauth_release_fixture() -> (
    MeerkatMachine,
    SessionId,
    Input,
    Arc<EphemeralTokenStore>,
    meerkat_core::handles::GeneratedAuthLeaseHandle,
    Arc<crate::handles::RuntimeAuthLeaseHandle>,
    Arc<crate::handles::FailingOAuthSnapshotStore>,
    Arc<crate::handles::RuntimeOAuthFlowHandle>,
) {
    use meerkat_auth_core::oauth_flow::OAuthFlowAuthority;
    let (machine, session, prompt) = pending().await;
    let raw = Arc::new(crate::handles::RuntimeAuthLeaseHandle::new());
    machine.set_runtime_auth_lease_handle(raw.clone()).unwrap();
    let (vault, handle) = commit(&machine, &prompt).await;
    let store = Arc::new(crate::handles::FailingOAuthSnapshotStore::default());
    let store_dyn: Arc<dyn crate::store::RuntimeStore> = store.clone();
    let flows = Arc::new(
        crate::handles::RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            std::time::Duration::from_secs(600),
            raw.clone(),
            &store_dyn,
        ),
    );
    flows
        .start(
            identity(&prompt),
            meerkat_auth_core::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
            "http://127.0.0.1/callback".into(),
            "synthetic-verifier".into(),
        )
        .unwrap();
    (machine, session, prompt, vault, handle, raw, store, flows)
}

async fn unrelated_pending(machine: &MeerkatMachine, original: &Input) -> (SessionId, Input) {
    let session = SessionId::new();
    machine.register_session(session.clone()).await.unwrap();
    let runtime = machine
        .sessions
        .read()
        .await
        .get(&session)
        .unwrap()
        .runtime_id
        .clone();
    let mut prompt = original.clone();
    prompt.header_mut().id = meerkat_core::InputId::new();
    let ingress = prompt.header().ingress_context.as_ref().unwrap().clone();
    let mut candidate = prompt
        .header()
        .authority_association
        .as_ref()
        .unwrap()
        .candidate()
        .clone();
    let old = candidate.controller_model.as_ref().unwrap();
    candidate.controller_model = Some(ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: old.model().into(),
            provider: old.provider(),
            self_hosted_server_id: old.self_hosted_server_id().map(str::to_owned),
            auth_binding: old.auth_binding().cloned(),
            provider_params: None,
        },
        meerkat_core::AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
            realm: meerkat_core::RealmId::parse("native-test").unwrap(),
            account: meerkat_core::CredentialAccountId::parse(format!(
                "other-{}",
                uuid::Uuid::new_v4()
            ))
            .unwrap(),
        }),
        old.backend_profile_id().into(),
        old.backend_kind().into(),
    ));
    candidate.target.logical_runtime = EvidenceId::new(runtime.to_string()).unwrap();
    let selected = candidate.controller_model.clone().unwrap();
    prompt.header_mut().authority_association =
        Some(InputAuthorityAssociation::new(candidate).unwrap());
    let actual = NativeIngressContext::from_trusted_ingress(
        &prompt,
        ingress.requester().clone(),
        ingress.ingress_actor().clone(),
        ingress.realm().clone(),
        ingress.authentication().clone(),
    )
    .unwrap()
    .with_controller_client(
        &prompt,
        ControllerModelClient::new(selected.clone(), Arc::new(SelectedClient(selected))),
    )
    .unwrap();
    (session, prompt.with_ingress_context(actual).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_store_wait_excludes_only_same_lease_and_refuses_nonempty_replacement() {
    use meerkat_core::handles::AuthLeaseHandle;
    let (machine, session, prompt, _vault, handle, raw, store, _flows) =
        oauth_release_fixture().await;
    let (other_session, other_prompt) = unrelated_pending(&machine, &prompt).await;
    let (_other_vault, _) = commit(&machine, &other_prompt).await;
    let key = LeaseKey::from_credential_identity(&identity(&prompt));
    let replacement = Arc::new(crate::handles::RuntimeAuthLeaseHandle::new());
    replacement.acquire_lease(&key, u64::MAX).unwrap();
    let before_replacement = replacement.snapshot(&key);
    store.block_next_oauth_persist();
    let release_gate = ReleaseGate(store.clone());
    let worker = std::thread::spawn({
        let handle = handle.clone();
        let key = key.clone();
        move || handle.release_lease(&key)
    });
    store.wait_for_blocked_oauth_persist();
    // This signal comes from the actual OAuth durable update, not a pre-call hook.
    assert!(matches!(
        machine.set_runtime_auth_lease_handle(replacement.clone()),
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::ReplacementNotEmpty
        })
    ));
    assert_eq!(replacement.snapshot(&key), before_replacement);
    assert!(Arc::ptr_eq(
        &machine.generated_auth_lease_handle().clone_handle(),
        &handle.clone_handle()
    ));
    machine.set_runtime_auth_lease_handle(raw).unwrap(); // same handle is a no-op, even while busy
    let refused = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        attempt(&machine, &session, prompt.clone()),
    )
    .await
    .expect("native locks are not retained across OAuth I/O");
    assert!(matches!(
        refused,
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::Busy
        })
    ));
    assert_unaccepted(&machine, &session, prompt.id()).await;
    let (_id, run, context) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        stage_controller_input(&machine, &other_session, other_prompt),
    )
    .await
    .expect("unrelated native session progresses");
    assert!(context.controller_client().is_some());
    let driver = machine
        .sessions
        .read()
        .await
        .get(&other_session)
        .unwrap()
        .driver
        .clone();
    let locked = driver.lock().await;
    let authority = locked.shared_dsl_authority();
    dsl::MeerkatMachineMutator::apply(
        &mut *authority.lock().unwrap(),
        dsl::MeerkatMachineInput::RunCompleted {
            run_id: dsl::RunId::from_domain(&run),
        },
    )
    .unwrap();
    drop(locked);
    drop(release_gate);
    worker.join().unwrap().unwrap();
    assert!(!handle.snapshot(&key).credential_present);
    assert!(matches!(
        attempt(&machine, &session, prompt).await,
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::CredentialUnusable { .. }
        })
    ));
}

#[tokio::test]
async fn failed_oauth_cleanup_preserves_credential_then_admission_and_retry_work() {
    let (machine, session, prompt, vault, handle, _raw, store, _flows) =
        oauth_release_fixture().await;
    let credential = identity(&prompt);
    let key = LeaseKey::from_credential_identity(&credential);
    let token_key = meerkat_core::auth::TokenKey::from_credential_identity(&credential);
    let before = vault.load(&token_key).await.unwrap();
    let snapshot = handle.snapshot(&key);
    store.fail_oauth_persist();
    let error = meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
        ProviderAuthPersistence::new(vault.clone(), Arc::new(InMemoryCoordinator::new())),
        handle.clone(),
        credential.clone(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, meerkat_core::auth::CredentialMutationError::AuthLifecycle(ref message)
        if message.contains("AuthLeaseReleaseObserver::release_oauth_flow_payloads"))
    );
    assert_eq!(vault.load(&token_key).await.unwrap(), before);
    assert_eq!(handle.snapshot(&key), snapshot);
    assert_eq!(
        handle
            .resolve_credential_use_admission(
                &key,
                meerkat_core::handles::CredentialUseIntent::HoldAuthority
            )
            .unwrap(),
        CredentialUseDisposition::Authorized
    );
    attempt(&machine, &session, prompt).await.unwrap();
    // Retire the actual accepted row before retrying an administrative release.
    let driver = machine
        .sessions
        .read()
        .await
        .get(&session)
        .unwrap()
        .driver
        .clone();
    let mut locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &mut *locked else {
        panic!("storeless")
    };
    driver
        .abandon_all_non_terminal(crate::input_state::InputAbandonReason::Stopped)
        .unwrap();
    drop(locked);
    store.allow_oauth_persist();
    meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
        ProviderAuthPersistence::new(vault.clone(), Arc::new(InMemoryCoordinator::new())),
        handle.clone(),
        credential,
    )
    .await
    .unwrap();
    assert!(vault.load(&token_key).await.unwrap().is_none());
    assert!(!handle.snapshot(&key).credential_present);
}

#[tokio::test]
async fn guarded_release_rejects_other_key_before_observers_or_mutation() {
    let (machine, _session, prompt, _vault, handle, _raw, store, _flows) =
        oauth_release_fixture().await;
    let key = LeaseKey::from_credential_identity(&identity(&prompt));
    let other = LeaseKey::from_credential_identity(&meerkat_core::AuthCredentialIdentity::Account(
        meerkat_core::CredentialAccountRef {
            realm: meerkat_core::RealmId::parse("native-test").unwrap(),
            account: meerkat_core::CredentialAccountId::parse(format!(
                "guard-{}",
                uuid::Uuid::new_v4()
            ))
            .unwrap(),
        },
    ));
    let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&other).await;
    let snapshot = handle.snapshot(&key);
    store.fail_oauth_persist();
    let error = handle.release_lease_with_guard(&key, &guard).unwrap_err();
    assert_eq!(
        error.context,
        "AuthLeaseHandle::release_lease_with_guard::LeaseMismatch"
    );
    assert_eq!(
        error.kind,
        meerkat_core::handles::DslRejectionKind::NoMatchingTransition
    );
    assert_eq!(handle.snapshot(&key), snapshot);
    assert!(Arc::ptr_eq(
        &machine.generated_auth_lease_handle().clone_handle(),
        &handle.clone_handle()
    ));
}

async fn generic_host_pending() -> (
    MeerkatMachine,
    SessionId,
    Input,
    meerkat_core::handles::GeneratedAuthLeaseHandle,
) {
    let machine = MeerkatMachine::ephemeral();
    let foreign = crate::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
        Arc::new(crate::handles::RuntimeAuthLeaseHandle::new()),
    )
    .unwrap();
    // This host has access to a different valid generated owner. Its policy
    // callbacks must not select that owner for this machine's admission.
    let host = Arc::new(crate::input_authority::tests::TestIngress::new(
        foreign.clone(),
    ));
    let machine = machine.with_native_work_authorization_host(host).unwrap();
    let session = SessionId::new();
    machine.register_session(session.clone()).await.unwrap();
    let runtime = machine
        .sessions
        .read()
        .await
        .get(&session)
        .unwrap()
        .runtime_id
        .clone();
    let mut prompt = input("caller");
    let original = prompt.header().ingress_context.as_ref().unwrap().clone();
    let mut candidate = prompt
        .header()
        .authority_association
        .as_ref()
        .unwrap()
        .candidate()
        .clone();
    candidate.target.logical_runtime = EvidenceId::new(runtime.to_string()).unwrap();
    let selection = candidate.controller_model.clone().unwrap();
    prompt.header_mut().authority_association =
        Some(InputAuthorityAssociation::new(candidate).unwrap());
    let context = NativeIngressContext::from_trusted_ingress(
        &prompt,
        original.requester().clone(),
        original.ingress_actor().clone(),
        original.realm().clone(),
        original.authentication().clone(),
    )
    .unwrap()
    .with_controller_client(
        &prompt,
        ControllerModelClient::new(selection.clone(), Arc::new(SelectedClient(selection))),
    )
    .unwrap();
    let prompt = prompt.with_ingress_context(context).unwrap();
    (machine, session, prompt, foreign)
}

#[tokio::test]
async fn generic_host_cannot_select_foreign_credential_owner() {
    let (machine, session, prompt, foreign) = generic_host_pending().await;
    let key = LeaseKey::from_credential_identity(&identity(&prompt));
    assert_eq!(
        foreign
            .resolve_credential_use_admission(
                &key,
                meerkat_core::handles::CredentialUseIntent::HoldAuthority,
            )
            .unwrap(),
        CredentialUseDisposition::Authorized
    );
    assert!(matches!(
        attempt(&machine, &session, prompt.clone()).await,
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::CredentialUnusable { .. }
        })
    ));
    assert_unaccepted(&machine, &session, prompt.id()).await;
    let (_store, actual) = commit(&machine, &prompt).await;
    assert!(matches!(
        attempt(&machine, &session, prompt).await.unwrap(),
        crate::accept::AcceptOutcome::Accepted { .. }
    ));
    assert_release_veto(&actual, &key, "NativeControllerCredentialRelease::InUse");
    foreign.release_lease(&key).unwrap();
    assert_eq!(
        actual
            .resolve_credential_use_admission(
                &key,
                meerkat_core::handles::CredentialUseIntent::HoldAuthority,
            )
            .unwrap(),
        CredentialUseDisposition::Authorized
    );
}

#[tokio::test]
async fn generic_host_owner_replacement_is_rechecked_at_native_entry() {
    let (machine, session, prompt, _foreign) = generic_host_pending().await;
    let (_store, _actual) = commit(&machine, &prompt).await;
    let custody = crate::meerkat_machine::credential_custody::NativeCredentialCustody::try_acquire(
        &machine.native_work_authorization_host,
        &prompt,
    )
    .unwrap();
    machine
        .set_runtime_auth_lease_handle(Arc::new(crate::handles::RuntimeAuthLeaseHandle::new()))
        .unwrap();
    assert!(matches!(
        custody.validate(&machine.native_work_authorization_host, &prompt),
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::AuthorityChanged
        })
    ));
    assert_unaccepted(&machine, &session, prompt.id()).await;
    drop(custody);
    assert!(matches!(
        attempt(&machine, &session, prompt.clone()).await,
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::CredentialUnusable { .. }
        })
    ));
    let (_store, _replacement) = commit(&machine, &prompt).await;
    assert!(matches!(
        attempt(&machine, &session, prompt).await.unwrap(),
        crate::accept::AcceptOutcome::Accepted { .. }
    ));
}

#[tokio::test]
async fn detached_authorization_attachment_cannot_keep_owner_alive() {
    let (machine, _session, prompt, _foreign) = generic_host_pending().await;
    let (_store, _actual) = commit(&machine, &prompt).await;
    let slot = machine.native_work_authorization_host.clone();
    let custody = crate::meerkat_machine::credential_custody::NativeCredentialCustody::try_acquire(
        &slot, &prompt,
    )
    .unwrap();
    drop(machine);
    assert!(matches!(
        custody.validate(&slot, &prompt),
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::AuthorityUnavailable
        })
    ));
}

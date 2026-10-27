//! Real credential owner and native accepted-row custody controls.
//! API-dependent source tests. No execution is claimed by this packet.
use super::*;
use crate::traits::{ControllerReadinessFailure, RuntimeDriverError};
use meerkat_auth_core::{EphemeralTokenStore, InMemoryCoordinator};
use meerkat_core::auth::{PersistedTokens, ProviderAuthPersistence, TokenStore};
use meerkat_core::handles::{CredentialUseDisposition, LeaseKey};

async fn pending() -> (MeerkatMachine, SessionId, Input) {
    pending_requiring(meerkat_core::ControllerCredentialRequirement::Required).await
}

/// A pending governed input whose selected controller's resolved connection
/// reports `requirement`.
async fn pending_requiring(
    requirement: meerkat_core::ControllerCredentialRequirement,
) -> (MeerkatMachine, SessionId, Input) {
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
    candidate.controller_model = Some(
        ControllerModelSelection::new(
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
        )
        .with_credential_requirement(requirement),
    );
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
    expected_kind: meerkat_core::handles::DslRejectionKind,
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
    assert_eq!(error.kind, expected_kind);
    assert_eq!(error.context, expected_context);
}

const IN_USE: meerkat_core::handles::DslRejectionKind =
    meerkat_core::handles::DslRejectionKind::NoMatchingTransition;
const BUSY: meerkat_core::handles::DslRejectionKind =
    meerkat_core::handles::DslRejectionKind::ControllerReadinessRefused(
        ControllerReadinessFailure::Busy,
    );

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

/// An explicitly authless controller (its resolved connection holds no
/// credential material) is admitted with no credential lifecycle, and none
/// is synthesized for it. The same input with a credential-required
/// selection and no lease stays refused.
#[tokio::test]
async fn an_authless_controller_is_admitted_without_a_credential_lease() {
    let (machine, session, prompt) =
        pending_requiring(meerkat_core::ControllerCredentialRequirement::Required).await;
    let id = prompt.id().clone();
    assert!(matches!(
        attempt(&machine, &session, prompt).await,
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::CredentialUnusable {
                disposition: CredentialUseDisposition::LeaseAbsent
            }
        })
    ));
    assert_unaccepted(&machine, &session, &id).await;

    let (machine, session, prompt) =
        pending_requiring(meerkat_core::ControllerCredentialRequirement::Authless).await;
    let id = prompt.id().clone();
    let key = LeaseKey::from_credential_identity(&identity(&prompt));
    assert!(
        matches!(attempt(&machine, &session, prompt).await.unwrap(), crate::accept::AcceptOutcome::Accepted { input_id, .. } if input_id == id)
    );
    let snapshot = machine.generated_auth_lease_handle().snapshot(&key);
    assert_eq!(snapshot.phase, None, "no lease is synthesized");
    assert!(!snapshot.credential_present);
}

/// The selected client reports one selection while it is pinned, then
/// another: a controller that is no longer the pinned route.
struct SwitchingClient {
    model: String,
    selection: std::sync::Mutex<ControllerModelSelection>,
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl meerkat_core::AgentLlmClient for SwitchingClient {
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(self.selection.lock().unwrap().clone())
    }
    async fn stream_response(
        &self,
        _: &[meerkat_core::Message],
        _: &[Arc<meerkat_core::ToolDef>],
        _: u32,
        _: Option<f32>,
        _: Option<&meerkat_core::ProviderParamsOverride>,
    ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
        Err(meerkat_core::AgentError::ConfigError(
            "source-only fixture never calls a model".into(),
        ))
    }
    fn provider(&self) -> meerkat_core::Provider {
        self.selection.lock().unwrap().provider()
    }
    fn model(&self) -> &str {
        &self.model
    }
}

/// An authless selection is honored only while the live selected client
/// still reports it: once the client reports a credential-required route,
/// the pinned authless claim is refused, not used to skip the lease.
#[tokio::test]
async fn an_authless_claim_the_live_client_no_longer_reports_is_refused() {
    let (machine, session, prompt) =
        pending_requiring(meerkat_core::ControllerCredentialRequirement::Authless).await;
    let ingress = prompt.header().ingress_context.as_ref().unwrap().clone();
    let pinned = ingress.controller_client().unwrap().selection().clone();
    let client = Arc::new(SwitchingClient {
        model: pinned.model().to_owned(),
        selection: std::sync::Mutex::new(pinned.clone()),
    });
    let context = NativeIngressContext::from_trusted_ingress(
        &prompt,
        ingress.requester().clone(),
        ingress.ingress_actor().clone(),
        ingress.realm().clone(),
        ingress.authentication().clone(),
    )
    .unwrap()
    .with_controller_client(
        &prompt,
        ControllerModelClient::new(pinned.clone(), client.clone()),
    )
    .unwrap();
    let prompt = prompt.with_ingress_context(context).unwrap();
    *client.selection.lock().unwrap() = pinned
        .clone()
        .with_credential_requirement(meerkat_core::ControllerCredentialRequirement::Required);
    let id = prompt.id().clone();
    assert!(matches!(
        attempt(&machine, &session, prompt).await,
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::AuthorityChanged
        })
    ));
    assert_unaccepted(&machine, &session, &id).await;
}

/// `selection` on the synthetic env-var default route, as the provider
/// factory pins it: no auth binding is recorded (the env default never
/// becomes durable identity), the credential is the one the provider
/// registry derives for the `env_default` binding, and its inline secret
/// keeps the route credential-required. `recorded_binding` instead records a
/// configured binding for that credential, the shape of every route that
/// publishes a lease. The per-call profile keeps each fixture's lease key
/// its own.
fn on_env_default_route(
    selection: &ControllerModelSelection,
    recorded_binding: bool,
) -> ControllerModelSelection {
    let env_default = meerkat_core::AuthBindingRef {
        realm: meerkat_core::RealmId::parse("env_default").unwrap(),
        binding: meerkat_core::BindingId::parse("default").unwrap(),
        profile: Some(
            meerkat_core::ProfileId::parse(format!("custody-{}", uuid::Uuid::new_v4())).unwrap(),
        ),
        origin: meerkat_core::BindingOrigin::SyntheticEnvDefault,
    };
    let credential = meerkat_core::AuthCredentialIdentity::from_auth_binding(&env_default);
    ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: selection.model().into(),
            provider: selection.provider(),
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: recorded_binding.then_some(meerkat_core::AuthBindingRef {
                origin: meerkat_core::BindingOrigin::Configured,
                ..env_default
            }),
        },
        credential,
        selection.backend_profile_id().into(),
        selection.backend_kind().into(),
    )
    .with_credential_requirement(meerkat_core::ControllerCredentialRequirement::Required)
}

/// A pending governed input whose selected controller is on the env-var
/// default route (see [`on_env_default_route`]).
async fn pending_env_default(recorded_binding: bool) -> (MeerkatMachine, SessionId, Input) {
    let (configuration, mut prompt, _, _) = mutable_controller_configuration(true);
    let mut candidate = prompt
        .header()
        .authority_association
        .as_ref()
        .unwrap()
        .candidate()
        .clone();
    let selection = candidate.controller_model.as_ref().unwrap();
    candidate.controller_model = Some(on_env_default_route(selection, recorded_binding));
    prompt.header_mut().authority_association =
        Some(InputAuthorityAssociation::new(candidate).unwrap());
    pending_controller_input(configuration, prompt).await
}

/// A stock host's env-var key (the synthetic env default) has no credential
/// lifecycle to hold: the factory publishes no lease for it by design, and
/// none can ever exist. Governed admission accepts the route without one and
/// synthesizes none.
#[tokio::test]
async fn the_env_default_controller_is_admitted_without_a_credential_lease() {
    let (machine, session, prompt) = pending_env_default(false).await;
    let id = prompt.id().clone();
    let key = LeaseKey::from_credential_identity(&identity(&prompt));
    let outcome = attempt(&machine, &session, prompt).await;
    assert!(
        matches!(&outcome, Ok(crate::accept::AcceptOutcome::Accepted { input_id, .. }) if *input_id == id),
        "the env-default controller must be admitted without a lease: {outcome:?}"
    );
    let snapshot = machine.generated_auth_lease_handle().snapshot(&key);
    assert_eq!(snapshot.phase, None, "no lease is published or synthesized");
    assert!(!snapshot.credential_present);
}

/// A selection that records an auth binding is a configured route, even for
/// a credential named in the env_default realm: it still needs its lease.
#[tokio::test]
async fn a_recorded_binding_on_the_env_default_credential_still_needs_its_lease() {
    let (machine, session, prompt) = pending_env_default(true).await;
    let id = prompt.id().clone();
    let outcome = attempt(&machine, &session, prompt).await;
    assert!(
        matches!(
            &outcome,
            Err(RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::CredentialUnusable {
                    disposition: CredentialUseDisposition::LeaseAbsent
                }
            })
        ),
        "a recorded binding is lease-bound: {outcome:?}"
    );
    assert_unaccepted(&machine, &session, &id).await;
}

/// The env-default route is honored without a lease only while the live
/// selected client still reports it: once the client reports a configured
/// route, the pinned env-default claim is refused, not used to skip the
/// lease.
#[tokio::test]
async fn an_env_default_claim_the_live_client_no_longer_reports_is_refused() {
    let (machine, session, prompt) = pending_env_default(false).await;
    let ingress = prompt.header().ingress_context.as_ref().unwrap().clone();
    let pinned = ingress.controller_client().unwrap().selection().clone();
    let client = Arc::new(SwitchingClient {
        model: pinned.model().to_owned(),
        selection: std::sync::Mutex::new(pinned.clone()),
    });
    let context = NativeIngressContext::from_trusted_ingress(
        &prompt,
        ingress.requester().clone(),
        ingress.ingress_actor().clone(),
        ingress.realm().clone(),
        ingress.authentication().clone(),
    )
    .unwrap()
    .with_controller_client(
        &prompt,
        ControllerModelClient::new(pinned.clone(), client.clone()),
    )
    .unwrap();
    let prompt = prompt.with_ingress_context(context).unwrap();
    *client.selection.lock().unwrap() = on_env_default_route(&pinned, true);
    let id = prompt.id().clone();
    let outcome = attempt(&machine, &session, prompt).await;
    assert!(
        matches!(
            &outcome,
            Err(RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::AuthorityChanged
            })
        ),
        "a pinned env-default claim must not outlive its route: {outcome:?}"
    );
    assert_unaccepted(&machine, &session, &id).await;
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
    assert_release_veto(
        &handle,
        &key,
        "NativeControllerCredentialRelease::InUse",
        IN_USE,
    );
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
    assert_release_veto(
        &old,
        &key,
        "NativeControllerCredentialRelease::InUse",
        IN_USE,
    );
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
        assert_release_veto(
            &handle,
            &key,
            "NativeControllerCredentialRelease::InUse",
            IN_USE,
        );
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
    assert_release_veto(
        &handle,
        &key,
        "NativeControllerCredentialRelease::Busy",
        BUSY,
    );
    assert_eq!(handle.snapshot(&key), before);
    drop(held);
}

/// Wait until `machine`'s session map can be taken exclusively, proving a
/// refused publication released its native custody before backing off.
async fn session_map_is_free(machine: &MeerkatMachine) -> bool {
    for _ in 0..200 {
        if machine.sessions.try_write().is_ok() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    false
}

#[tokio::test]
async fn busy_publication_releases_custody_while_waiting_then_publishes() {
    let (machine, session, prompt) = pending().await;
    let (_store, handle) = commit(&machine, &prompt).await;
    let unrelated =
        meerkat_core::AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
            realm: meerkat_core::RealmId::parse("native-test").unwrap(),
            account: meerkat_core::CredentialAccountId::parse(format!(
                "busy-{}",
                uuid::Uuid::new_v4()
            ))
            .unwrap(),
        });
    let other_key = LeaseKey::from_credential_identity(&unrelated);
    let tokens = PersistedTokens::api_key("synthetic-busy");
    let before = handle.snapshot(&other_key);
    let driver = machine
        .sessions
        .read()
        .await
        .get(&session)
        .unwrap()
        .driver
        .clone();
    let held = driver.lock().await;
    let publication = meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &handle,
        &unrelated,
        &tokens,
        tokio::time::Instant::now() + std::time::Duration::from_secs(10),
    );
    let contention = async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            handle.snapshot(&other_key),
            before,
            "refused attempts publish nothing"
        );
        assert!(
            session_map_is_free(&machine).await,
            "custody is released while waiting"
        );
        drop(held);
    };
    let (published, ()) = tokio::join!(publication, contention);
    published.expect("publication after the driver is free");
    assert!(handle.snapshot(&other_key).credential_present);
}

#[tokio::test]
async fn busy_publication_rechecks_references_after_fresh_custody() {
    let (machine, session, prompt) = pending().await;
    let credential = identity(&prompt);
    let key = LeaseKey::from_credential_identity(&credential);
    let (_store, handle) = commit(&machine, &prompt).await;
    // Busy is decided before references are read; the referencing input is
    // invisible to a refused attempt and must be found by the fresh one.
    stage_controller_input(&machine, &session, prompt).await;
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
    let tokens = PersistedTokens::api_key("synthetic-replacement");
    let publication = meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &handle,
        &credential,
        &tokens,
        tokio::time::Instant::now() + std::time::Duration::from_secs(10),
    );
    let contention = async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            handle.snapshot(&key),
            before,
            "refused attempts publish nothing"
        );
        drop(held);
    };
    let (published, ()) = tokio::join!(publication, contention);
    let Err(meerkat_core::BoundedLifecyclePublicationError::Refused(error)) = published else {
        panic!("a fresh attempt must see the reference: {published:?}");
    };
    assert_eq!(error.context, "NativeControllerCredentialRelease::InUse");
    assert_eq!(error.controller_readiness_refusal(), None);
    assert_eq!(handle.snapshot(&key), before);
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
            std::time::Duration::from_mins(10),
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
    let host = Arc::new(crate::input_authority::tests::TestIngress::isolated(
        foreign.clone(),
    ));
    let machine = machine
        .with_native_work_authorization_host(host.clone())
        .unwrap();
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
    let mut prompt = host.input("caller");
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
    assert_release_veto(
        &actual,
        &key,
        "NativeControllerCredentialRelease::InUse",
        IN_USE,
    );
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

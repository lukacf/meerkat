//! Actual memory-backed persistent construction and custody controls.
//! These tests do not qualify restart, SQLite, or unloaded administrative scope.
use super::*;
use crate::driver::persistent::PersistentRuntimeDriver;
use crate::store::{InMemoryRuntimeStore, RuntimeStore};
use crate::traits::ControllerReadinessFailure;
use meerkat_core::BlobStore;
use meerkat_store::MemoryBlobStore;

fn persistent(store: &InMemoryRuntimeStore) -> MeerkatMachine {
    let runtime_store: Arc<dyn RuntimeStore> = Arc::new(store.clone());
    MeerkatMachine::persistent(runtime_store, Arc::new(MemoryBlobStore::new()))
        .expect("ordinary memory persistent owner")
}

fn try_governed(store: &InMemoryRuntimeStore) -> Result<MeerkatMachine, RuntimeDriverError> {
    MeerkatMachine::persistent_with_local_grant_authorization(
        Arc::new(store.clone()),
        Some(Arc::new(MemoryBlobStore::new())),
        configuration().0,
    )
}

fn governed(store: &InMemoryRuntimeStore) -> MeerkatMachine {
    try_governed(store).expect("real memory persistent owner supports configured governance")
}

async fn governed_after_quiescence(store: &InMemoryRuntimeStore) -> MeerkatMachine {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match try_governed(store) {
                Ok(machine) => return machine,
                Err(RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: ControllerReadinessFailure::Busy,
                }) => tokio::task::yield_now().await,
                Err(error) => panic!("quiescent construction failed: {error:?}"),
            }
        }
    })
    .await
    .expect("actual detached owners must finish before custody can reopen")
}

fn expect_busy(result: Result<MeerkatMachine, RuntimeDriverError>) {
    match result {
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason: ControllerReadinessFailure::Busy,
        }) => {}
        Err(error) => panic!("expected exact backend custody contention, got {error:?}"),
        Ok(_) => panic!("a second execution owner entered exclusive governed scope"),
    }
}

#[tokio::test]
async fn memory_persistent_shared_owners_precede_exclusive_setup() {
    let store = InMemoryRuntimeStore::new();
    let first = persistent(&store);
    let second = persistent(&store);
    first
        .register_session(SessionId::new())
        .await
        .expect("first ordinary owner");
    second
        .register_session(SessionId::new())
        .await
        .expect("second ordinary owner");
    expect_busy(try_governed(&store));
    drop(first);
    expect_busy(try_governed(&store));
    drop(second);
    let exclusive = governed_after_quiescence(&store).await;
    exclusive
        .register_session(SessionId::new())
        .await
        .expect("quiescent store can reopen");
}

#[tokio::test]
async fn memory_persistent_exclusive_owner_precedes_ordinary_setup() {
    let store = InMemoryRuntimeStore::new();
    let exclusive = governed(&store);
    let session = SessionId::new();
    expect_busy(MeerkatMachine::persistent(
        Arc::new(store.clone()),
        Arc::new(MemoryBlobStore::new()),
    ));
    assert!(
        exclusive.sessions.read().await.is_empty(),
        "no entry before custody"
    );
    assert!(
        store
            .load_input_states(&LogicalRuntimeId::for_session(&session))
            .await
            .unwrap()
            .is_empty()
    );
    drop(exclusive);
    persistent(&store)
        .register_session(session)
        .await
        .expect("fresh ordinary owner after quiescence");
}

#[tokio::test]
async fn memory_persistent_detached_driver_retains_backend_claim() {
    let store = InMemoryRuntimeStore::new();
    let machine = governed(&store);
    let session = SessionId::new();
    machine.register_session(session.clone()).await.unwrap();
    let retained_driver = Arc::clone(&machine.sessions.read().await.get(&session).unwrap().driver);
    assert!(matches!(
        &*retained_driver.lock().await,
        DriverEntry::Persistent(_)
    ));
    drop(machine);
    expect_busy(try_governed(&store));
    drop(retained_driver);
    governed_after_quiescence(&store)
        .await
        .register_session(SessionId::new())
        .await
        .expect("last actual owner released custody");
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn memory_persistent_exported_provider_authority_retains_backend_claim() {
    use meerkat_auth_core::oauth_flow::OAuthProviderIdentity;

    let store = InMemoryRuntimeStore::new();
    // OAuth owns a Weak store reference. Retain the actual Arc supplied to the
    // machine so the escaped capability still has its real persistence sink.
    let runtime_store: Arc<dyn RuntimeStore> = Arc::new(store.clone());
    let machine = MeerkatMachine::persistent_with_local_grant_authorization(
        Arc::clone(&runtime_store),
        Some(Arc::new(MemoryBlobStore::new())),
        configuration().0,
    )
    .unwrap();
    assert!(machine.sessions.read().await.is_empty());
    let authority = machine.provider_auth_runtime_authority();
    let oauth = authority.oauth_flow_authority();
    drop(machine);

    // Exercise only the real local login-flow owner, with no provider request.
    // The flow is retired before testing lifetime exclusion, so an outstanding
    // login attempt cannot accidentally become the exclusion oracle.
    let target = meerkat_core::AuthCredentialIdentity::Binding(meerkat_core::AuthBindingRef {
        realm: meerkat_core::RealmId::parse("native-test").unwrap(),
        binding: meerkat_core::BindingId::parse("provider-authority-lifetime").unwrap(),
        profile: None,
        origin: meerkat_core::connection::BindingOrigin::Configured,
    });
    let provider = OAuthProviderIdentity::OpenAiChatGpt;
    let redirect = "http://127.0.0.1/callback";
    let before = store.auth_oauth_flow_store_calls();
    let state = oauth
        .start(
            target.clone(),
            provider.into(),
            redirect.into(),
            "synthetic-lifetime-verifier".into(),
        )
        .expect("exported owner remains usable after the machine drops");
    let flow = oauth
        .verify(&state, &target, provider.into(), redirect)
        .expect("the same exported owner verifies its actual flow");
    assert_eq!(flow.target, target);
    assert_eq!(flow.pkce_verifier, "synthetic-lifetime-verifier");
    assert!(
        store.auth_oauth_flow_store_calls() > before,
        "the escaped owner used the actual runtime store"
    );
    assert!(store.load_auth_oauth_flow_snapshot().unwrap().is_some());
    oauth
        .expire(&state, &target, provider.into(), redirect)
        .expect("retire the login attempt while retaining the live capability");

    // An unrelated ordinary service must remain usable while this authority
    // holds custody; a process-global exclusion cannot satisfy the test.
    let independent_store = InMemoryRuntimeStore::new();
    let independent = persistent(&independent_store);
    let independent_session = SessionId::new();
    independent
        .register_session(independent_session.clone())
        .await
        .expect("independent ordinary service stays usable");

    expect_busy(try_governed(&store));
    drop(authority);
    // A capability extracted from the pair retains the same lifetime duty.
    expect_busy(try_governed(&store));
    drop(oauth);
    let reopened = governed_after_quiescence(&store).await;
    assert!(reopened.sessions.read().await.is_empty());
    independent
        .unregister_session(&independent_session)
        .await
        .unwrap();
    drop(reopened);
    drop(runtime_store);
}

#[cfg(not(target_arch = "wasm32"))]
mod exported_authority_escapes {
    use super::*;
    use meerkat_auth_core::oauth_flow::OAuthProviderIdentity;
    use meerkat_core::handles::{AuthLeaseHandle, AuthLeasePhase, LeaseKey};

    async fn fixture() -> (InMemoryRuntimeStore, Arc<dyn RuntimeStore>, MeerkatMachine) {
        let store = InMemoryRuntimeStore::new();
        // Keep the exact store Arc alive independently of the machine. OAuth
        // retains only a Weak reference to its persistence sink.
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(store.clone());
        let machine = MeerkatMachine::persistent_with_local_grant_authorization(
            Arc::clone(&runtime_store),
            Some(Arc::new(MemoryBlobStore::new())),
            configuration().0,
        )
        .unwrap();
        assert!(machine.sessions.read().await.is_empty());
        (store, runtime_store, machine)
    }

    fn target(binding: &str) -> meerkat_core::AuthCredentialIdentity {
        meerkat_core::AuthCredentialIdentity::Binding(meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("native-test").unwrap(),
            binding: meerkat_core::BindingId::parse(binding).unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        })
    }

    fn exercise_credential(authority: &dyn AuthLeaseHandle, binding: &str) {
        let key = LeaseKey::from_credential_identity(&target(binding));
        authority
            .acquire_lease(&key, u64::MAX)
            .expect("escaped credential authority remains usable");
        assert_eq!(authority.snapshot(&key).phase, Some(AuthLeasePhase::Valid));
        authority.release_lease(&key).unwrap();
        // The public snapshot maps the generated Released phase to None.
        assert_eq!(authority.snapshot(&key).phase, None);
    }

    fn expect_retained_claim(store: &InMemoryRuntimeStore) {
        // A global singleton cannot satisfy exclusion for this store.
        let independent_store = InMemoryRuntimeStore::new();
        let independent = governed(&independent_store);
        expect_busy(try_governed(store));
        drop(independent);
    }

    async fn expect_released_claim(store: &InMemoryRuntimeStore) {
        let reopened = governed_after_quiescence(store).await;
        assert!(reopened.sessions.read().await.is_empty());
    }

    #[tokio::test]
    async fn memory_persistent_direct_oauth_export_retains_backend_claim() {
        let (store, _runtime_store, machine) = fixture().await;
        let oauth = machine.oauth_flow_authority();
        drop(machine);

        let target = target("direct-oauth-lifetime");
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect = "http://127.0.0.1/callback";
        let before = store.auth_oauth_flow_store_calls();
        let state = oauth
            .start(
                target.clone(),
                provider.into(),
                redirect.into(),
                "synthetic-direct-oauth-verifier".into(),
            )
            .unwrap();
        assert_eq!(
            oauth
                .verify(&state, &target, provider.into(), redirect)
                .unwrap()
                .target,
            target
        );
        oauth
            .expire(&state, &target, provider.into(), redirect)
            .unwrap();
        assert!(store.auth_oauth_flow_store_calls() > before);
        // The flow is retired; the live authority itself must retain custody.
        expect_retained_claim(&store);
        drop(oauth);
        expect_released_claim(&store).await;
    }

    #[tokio::test]
    async fn memory_persistent_direct_auth_export_retains_backend_claim() {
        let (store, _runtime_store, machine) = fixture().await;
        let raw = machine.auth_lease_handle();
        drop(machine);

        exercise_credential(raw.as_ref(), "direct-auth-lifetime");
        expect_retained_claim(&store);
        drop(raw);
        expect_released_claim(&store).await;
    }

    #[tokio::test]
    async fn memory_persistent_generated_to_raw_auth_export_retains_backend_claim() {
        let (store, _runtime_store, machine) = fixture().await;
        let generated = machine.generated_auth_lease_handle();
        let generated_clone = generated.clone();
        drop(machine);
        drop(generated);

        exercise_credential(generated_clone.as_handle(), "generated-clone-lifetime");
        expect_retained_claim(&store);
        let raw = generated_clone.clone_handle();
        drop(generated_clone);

        exercise_credential(raw.as_ref(), "raw-auth-lifetime");
        expect_retained_claim(&store);
        drop(raw);
        expect_released_claim(&store).await;
    }

    #[tokio::test]
    async fn memory_persistent_nested_credential_export_retains_backend_claim() {
        let (store, _runtime_store, machine) = fixture().await;
        let oauth = machine.oauth_flow_authority();
        let credential = oauth
            .generated_credential_lifecycle()
            .expect("runtime OAuth exports its actual generated credential owner");
        drop(machine);
        drop(oauth);

        exercise_credential(credential.as_handle(), "nested-auth-lifetime");
        expect_retained_claim(&store);
        drop(credential);
        expect_released_claim(&store).await;
    }

    #[tokio::test]
    async fn memory_persistent_device_poll_export_retains_backend_claim() {
        let (store, _runtime_store, machine) = fixture().await;
        let oauth = machine.oauth_flow_authority();
        let target = target("device-poll-lifetime");
        let provider = OAuthProviderIdentity::GoogleCodeAssist;
        let device_code = "synthetic-lifetime-device-code";
        oauth
            .admit_device_code(
                target.clone(),
                provider,
                device_code.into(),
                std::time::Duration::from_secs(3600),
            )
            .unwrap();
        let poll = oauth
            .begin_device_code_poll(device_code, &target, provider)
            .unwrap();
        drop(machine);
        drop(oauth);

        assert!(poll.terminal_flow_state_is_authmachine_owned());
        let record = poll.verify().expect("escaped device poll remains usable");
        assert_eq!(record.target, target);
        assert_eq!(record.device_code, device_code);
        expect_retained_claim(&store);
        let before = store.auth_oauth_flow_store_calls();
        let consumed = poll
            .consume()
            .expect("finish through the retained runtime lifecycle");
        assert_eq!(consumed.target, target);
        assert!(store.auth_oauth_flow_store_calls() > before);
        expect_released_claim(&store).await;
    }
}

#[tokio::test]
async fn memory_persistent_direct_driver_participates_in_both_orders() {
    let store = InMemoryRuntimeStore::new();
    let runtime_store: Arc<dyn RuntimeStore> = Arc::new(store.clone());
    let blobs: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let direct = PersistentRuntimeDriver::new(
        LogicalRuntimeId::new("direct-before"),
        Arc::clone(&runtime_store),
        Arc::clone(&blobs),
    );
    expect_busy(try_governed(&store));
    drop(direct);
    let exclusive = governed(&store);
    let runtime = LogicalRuntimeId::new("direct-after");
    let mut blocked = PersistentRuntimeDriver::new(runtime.clone(), runtime_store, blobs);
    let result = blocked.accept_input(input("requester")).await;
    assert!(
        matches!(
            result,
            Err(RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::Busy,
            })
        ),
        "direct driver must refuse before applying native input: {result:?}"
    );
    assert!(store.load_input_states(&runtime).await.unwrap().is_empty());
    assert!(blocked.active_input_ids().is_empty());
    drop(blocked);
    drop(exclusive);
}

#[tokio::test]
async fn memory_persistent_scope_does_not_exclude_an_independent_store() {
    let first_store = InMemoryRuntimeStore::new();
    let second_store = InMemoryRuntimeStore::new();
    let first = governed(&first_store);
    let second = governed(&second_store);
    first.register_session(SessionId::new()).await.unwrap();
    second.register_session(SessionId::new()).await.unwrap();
}

async fn pending_persistent_input() -> (
    MeerkatMachine,
    InMemoryRuntimeStore,
    SessionId,
    Input,
    ResourceDomain,
) {
    let store = InMemoryRuntimeStore::new();
    let (configuration, mut prompt, domain) = configuration();
    let machine = MeerkatMachine::persistent_with_local_grant_authorization(
        Arc::new(store.clone()),
        Some(Arc::new(MemoryBlobStore::new())),
        configuration,
    )
    .unwrap();
    let session = SessionId::new();
    machine.register_session(session.clone()).await.unwrap();
    let runtime = LogicalRuntimeId::for_session(&session);
    let observed = Arc::clone(prompt.header().ingress_context.as_ref().unwrap());
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
    let ingress = NativeIngressContext::from_trusted_ingress(
        &prompt,
        observed.requester().clone(),
        observed.ingress_actor().clone(),
        observed.realm().clone(),
        observed.authentication().clone(),
    )
    .unwrap()
    .with_controller_client(
        &prompt,
        ControllerModelClient::new(selection.clone(), Arc::new(SelectedClient(selection))),
    )
    .unwrap();
    let prompt = prompt.with_ingress_context(ingress).unwrap();
    (machine, store, session, prompt, domain)
}

#[tokio::test]
async fn memory_persistent_admission_keeps_typed_readiness_before_any_row() {
    let (machine, store, session, prompt, _) = pending_persistent_input().await;
    let custody = crate::meerkat_machine::credential_custody::NativeCredentialCustody::acquire(
        &machine.native_work_authorization_host,
        &prompt,
        true,
    )
    .await
    .unwrap();
    let shared_driver = Arc::clone(&machine.sessions.read().await.get(&session).unwrap().driver);
    let mut driver = shared_driver.lock().await;
    let DriverEntry::Persistent(persistent) = &mut *driver else {
        panic!("actual persistent driver required")
    };
    persistent
        .inner_mut()
        .set_executor_work_authorization_support(true);
    let before = persistent
        .inner_ref()
        .shared_dsl_authority()
        .lock()
        .unwrap()
        .state()
        .clone();
    let resolved = persistent.resolve_admission(&prompt).unwrap();
    let result = persistent
        .accept_resolved_input_with_credential(prompt, resolved, &custody)
        .await;
    assert!(
        matches!(
            result,
            Err(RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::CredentialUnusable { .. },
            })
        ),
        "missing actual credential must remain readiness: {result:?}"
    );
    assert!(
        before
            == *persistent
                .inner_ref()
                .shared_dsl_authority()
                .lock()
                .unwrap()
                .state()
    );
    persistent
        .require_durability_ready()
        .expect("pre-mutation readiness is not a failed durable commit");
    assert!(persistent.active_input_ids().is_empty());
    drop(driver);
    assert!(
        store
            .load_input_states(&LogicalRuntimeId::for_session(&session))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn memory_persistent_actual_row_and_health_fence_prepared_operations() {
    let (machine, store, session, prompt, domain) = pending_persistent_input().await;
    install_owner_fixture_credential(&machine, &prompt);
    let id = prompt.id().clone();
    let custody = crate::meerkat_machine::credential_custody::NativeCredentialCustody::acquire(
        &machine.native_work_authorization_host,
        &prompt,
        true,
    )
    .await
    .unwrap();
    let shared_driver = Arc::clone(&machine.sessions.read().await.get(&session).unwrap().driver);
    let run = meerkat_core::RunId::new();
    let (context, health) = {
        let mut driver = shared_driver.lock().await;
        let DriverEntry::Persistent(persistent) = &mut *driver else {
            panic!("actual persistent driver required")
        };
        persistent
            .inner_mut()
            .set_executor_work_authorization_support(true);
        let resolved = persistent.resolve_admission(&prompt).unwrap();
        let accepted = persistent
            .accept_resolved_input_with_credential(prompt, resolved, &custody)
            .await
            .unwrap();
        assert!(
            matches!(accepted, crate::accept::AcceptOutcome::Accepted { ref input_id, .. } if input_id == &id)
        );
        let context = persistent
            .inner_ref()
            .batch_work_authorization(&run, std::slice::from_ref(&id))
            .unwrap()
            .unwrap();
        persistent
            .contract_begin_run_authority(run.clone())
            .unwrap();
        persistent
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    vec![id.clone()],
                    run.clone(),
                ),
            )
            .unwrap();
        (context, persistent.durability_health_handle().unwrap())
    };
    drop(custody);
    let stored = store
        .load_input_state(&LogicalRuntimeId::for_session(&session), &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.state.authority_contributors.len(), 1);
    assert_eq!(stored.state.authority_contributors[0].input_id(), &id);
    assert!(
        stored.state.controller_client.is_none(),
        "stored history cannot recreate the live client"
    );
    let bound = binding(&context, &run, &domain);
    let check = context
        .authorization()
        .prepare(&bound)
        .expect("actual persistent accepted row and current grants");
    check.check_current(&bound).unwrap();
    assert!(health.mark_reload_required(
        "test_actual_commit_uncertainty",
        "store owner requires reload"
    ));
    assert!(matches!(
        check.check_current(&bound),
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    ));
    assert!(matches!(
        context.authorization().prepare(&bound),
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    ));
    check
        .observe(
            &bound,
            OperationObservation::Outcome(
                meerkat_core::authorization::OperationObservedOutcome::TransportError,
            ),
        )
        .expect("historical physical outcome remains recordable after health loss");
}

#[tokio::test]
async fn memory_persistent_failed_claim_does_not_touch_auth_store() {
    let independent = InMemoryRuntimeStore::new();
    let ordinary = persistent(&independent);
    assert!(
        independent.auth_oauth_flow_store_calls() > 0,
        "positive control observes actual auth rehydration"
    );

    let store = InMemoryRuntimeStore::new();
    let exclusive = governed(&store);
    // Both ordinary shared and governed persistent owners keep their scoped pair.
    for machine in [&ordinary, &exclusive] {
        let installed = machine.provider_auth_runtime_authority();
        let generated = installed.generated_auth_lease_handle();
        let concrete = (generated.as_handle() as &dyn std::any::Any)
            .downcast_ref::<crate::handles::RuntimeAuthLeaseHandle>()
            .unwrap();
        // Another outer Arc around a clone of the exact registry is a no-op. It
        // must not replace the installed scoped pair or its OAuth owner.
        machine
            .set_runtime_auth_lease_handle(Arc::new(concrete.clone()))
            .unwrap();
        assert!(matches!(
            machine.set_runtime_auth_lease_handle(Arc::new(
                crate::handles::RuntimeAuthLeaseHandle::new(),
            )),
            Err(RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::UnsupportedScope,
            })
        ));
        machine
            .set_auth_lease_handle_with_oauth_flow_authority(
                Arc::new(concrete.clone()),
                installed.oauth_flow_authority(),
            )
            .unwrap();
        let foreign_oauth = Arc::new(crate::handles::RuntimeOAuthFlowHandle::new_with_auth_lease(
            std::time::Duration::from_secs(60),
            Arc::new(concrete.clone()),
        ));
        assert!(matches!(
            machine.set_auth_lease_handle_with_oauth_flow_authority(
                Arc::new(concrete.clone()),
                foreign_oauth,
            ),
            Err(RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::UnsupportedScope,
            })
        ));
        let current = machine.provider_auth_runtime_authority();
        assert!(Arc::ptr_eq(
            &generated.clone_handle(),
            &current.generated_auth_lease_handle().clone_handle(),
        ));
        assert!(Arc::ptr_eq(
            &installed.oauth_flow_authority(),
            &current.oauth_flow_authority(),
        ));
    }
    drop(ordinary);
    let before = store.auth_oauth_flow_store_calls();
    // Distinct Arc allocations around clones exercise the existing process
    // fallback identity. They must not create or rebind a persistent auth owner.
    let blocked_with_blobs =
        MeerkatMachine::persistent(Arc::new(store.clone()), Arc::new(MemoryBlobStore::new()));
    let blocked_without_blobs = MeerkatMachine::persistent_without_blobs(Arc::new(store.clone()));
    assert_eq!(store.auth_oauth_flow_store_calls(), before);
    for blocked in [blocked_with_blobs, blocked_without_blobs] {
        expect_busy(blocked);
    }
    assert!(exclusive.sessions.read().await.is_empty());
    assert_eq!(store.auth_oauth_flow_store_calls(), before);
    drop(exclusive);
}

struct CurrentWriteFence;
impl crate::store::RuntimeStoreWriteFence for CurrentWriteFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), crate::store::RuntimeStoreError> + '_>,
    ) -> Result<crate::store::RuntimeStoreWriteFenceOutcome, crate::store::RuntimeStoreError> {
        operation()?;
        Ok(crate::store::RuntimeStoreWriteFenceOutcome::Applied)
    }
}

#[tokio::test]
async fn memory_persistent_failed_claim_refuses_conditional_registration_before_cas() {
    use crate::meerkat_machine::RuntimeSessionRegistrationOutcome;
    let store = InMemoryRuntimeStore::new();
    let exclusive = governed(&store);
    let session = SessionId::new();
    let before = store.machine_lifecycle_fenced_cas_calls();
    expect_busy(MeerkatMachine::persistent(
        Arc::new(store.clone()),
        Arc::new(MemoryBlobStore::new()),
    ));
    assert_eq!(
        store.machine_lifecycle_fenced_cas_calls(),
        before,
        "no physical CAS call before custody"
    );
    assert!(exclusive.sessions.read().await.is_empty());
    assert!(matches!(
        store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(&session))
            .await
            .unwrap(),
        crate::store::MachineLifecycleObservation::Missing
    ));
    drop(exclusive);

    let permitted = persistent(&store);
    let observed = permitted
        .observe_cold_runtime_lifecycle(&session)
        .await
        .unwrap();
    let outcome = permitted
        .register_session_if_runtime_lifecycle_current(observed, Arc::new(CurrentWriteFence))
        .await;
    assert!(
        matches!(outcome, RuntimeSessionRegistrationOutcome::Applied { .. }),
        "positive conditional registration must apply"
    );
    assert_eq!(store.machine_lifecycle_fenced_cas_calls(), before + 1);
    permitted.unregister_session(&session).await.unwrap();
}

struct ReleaseStoreGate(Arc<tokio::sync::Notify>);
impl Drop for ReleaseStoreGate {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

// The public attachment path installs the real ops persistence worker. No input
// is admitted by this fixture, so an unexpected executor apply is a test failure.
struct OpsPersistenceIdleExecutor;

#[async_trait::async_trait]
impl meerkat_core::lifecycle::core_executor::CoreExecutor for OpsPersistenceIdleExecutor {
    async fn apply(
        &mut self,
        _: meerkat_core::RunId,
        _: meerkat_core::lifecycle::run_primitive::RunPrimitive,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::lifecycle::core_executor::CoreExecutorError,
    > {
        panic!("ops persistence fixture must not execute input work")
    }

    async fn cancel_after_boundary(
        &mut self,
        _: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(
        &mut self,
        _: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        Ok(())
    }
}

#[tokio::test]
async fn memory_persistent_detached_ops_worker_retains_claim_until_real_store_completion() {
    use meerkat_core::ops_lifecycle::{
        OperationId, OperationKind, OperationSpec, OpsLifecycleRegistry,
    };
    let store = InMemoryRuntimeStore::new();
    let machine = Arc::new(governed(&store));
    let session = SessionId::new();
    machine.register_session(session.clone()).await.unwrap();
    machine.prepare_bindings(session.clone()).await.unwrap();
    machine
        .register_session_with_executor(session.clone(), Box::new(OpsPersistenceIdleExecutor))
        .await
        .expect("public executor attachment installs the actual persistence worker");
    let (registry, driver_lifetime) = {
        let sessions = machine.sessions.read().await;
        let entry = sessions.get(&session).unwrap();
        assert!(
            entry.ops_lifecycle_persistence_worker.is_some(),
            "actual worker installed"
        );
        (
            Arc::clone(&entry.ops_lifecycle),
            Arc::downgrade(&entry.driver),
        )
    };
    let operation = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session.clone(),
            display_name: "owned persistence".into(),
            source_label: "execution custody fixture".into(),
            operation_source: None,
            child_session_id: None,
            expect_peer_channel: false,
        })
        .unwrap();
    registry.provisioning_succeeded(&operation).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let release_on_exit = ReleaseStoreGate(Arc::clone(&release));
    store.block_next_ops_lifecycle_persist(Arc::clone(&entered), Arc::clone(&release));
    // The actual registry synchronously waits for the actual worker's store
    // completion. Keep that wait off the async executor used by this test.
    let mut write = tokio::task::spawn_blocking(move || {
        registry.fail_operation(&operation, "fixture terminal outcome".into())
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::select! {
            () = entered.notified() => {}
            result = &mut write => panic!("terminal mutation completed before the actual store barrier: {result:?}"),
        }
    }).await.expect("actual store write reached");
    drop(machine);
    // The attached runtime loop must finish its ordinary closed-channel stop.
    // Otherwise its retained driver could mask a missing worker-owned claim.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while driver_lifetime.strong_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the actual driver drops while the worker store write is still gated");
    expect_busy(try_governed(&store));
    drop(release_on_exit);
    tokio::time::timeout(std::time::Duration::from_secs(5), write)
        .await
        .expect("bounded actual write completion")
        .unwrap()
        .unwrap();
    let reopened = governed_after_quiescence(&store).await;
    assert!(
        store
            .load_ops_lifecycle(&LogicalRuntimeId::for_session(&session))
            .await
            .unwrap()
            .is_some(),
        "reopen follows physical persistence, not facade or waiter drop"
    );
    drop(reopened);
}

#[tokio::test]
async fn memory_persistent_direct_accept_holds_credential_until_real_row_commit() {
    let (machine, store, session, prompt, _) = pending_persistent_input().await;
    install_owner_fixture_credential(&machine, &prompt);
    let id = prompt.id().clone();
    let lease = meerkat_core::handles::LeaseKey::from_credential_identity(
        prompt
            .header()
            .ingress_context
            .as_ref()
            .unwrap()
            .controller_client()
            .unwrap()
            .selection()
            .credential(),
    );
    let driver = Arc::clone(&machine.sessions.read().await.get(&session).unwrap().driver);
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Persistent(persistent) = &mut *locked else {
            panic!("actual persistent driver")
        };
        persistent
            .inner_mut()
            .set_executor_work_authorization_support(true);
    }
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let release_on_exit = ReleaseStoreGate(Arc::clone(&release));
    store.block_next_atomic_input_persist(Arc::clone(&entered), release);
    // No test-owned credential guard: exercise the actual driver's acquisition
    // and continuation through its awaited physical row write.
    let accepted = tokio::spawn(async move {
        let mut locked = driver.lock().await;
        let DriverEntry::Persistent(persistent) = &mut *locked else {
            panic!("actual persistent driver")
        };
        persistent.accept_input(prompt).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .expect("actual row commit reached");
    let during = meerkat_core::try_acquire_auth_login_lifecycle_guard(&lease);
    let retained = during.is_none();
    drop(during);
    let before = store
        .load_input_state(&LogicalRuntimeId::for_session(&session), &id)
        .await
        .unwrap();
    drop(release_on_exit);
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), accepted)
        .await
        .expect("bounded actual row commit")
        .unwrap()
        .unwrap();
    assert!(
        retained,
        "the native lease spans the physical persistent acceptance commit"
    );
    assert!(
        before.is_none(),
        "barrier is before the actual store mutation"
    );
    assert!(
        matches!(outcome, crate::accept::AcceptOutcome::Accepted { ref input_id, .. } if input_id == &id)
    );
    assert!(
        store
            .load_input_state(&LogicalRuntimeId::for_session(&session), &id)
            .await
            .unwrap()
            .is_some()
    );
    let after = meerkat_core::try_acquire_auth_login_lifecycle_guard(&lease);
    assert!(
        after.is_some(),
        "completed acceptance released its own lease claim"
    );
    drop(after);
}

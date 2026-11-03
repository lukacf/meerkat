//! Native census and retained-task controls. Physical/Python closure and
//! actual installed-process Restore remain separate runtime acceptance.
use super::*;
use meerkat_core::lifecycle::{
    RunId,
    core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError},
    run_primitive::{RunApplyBoundary, RunPrimitive},
    run_receipt::RunBoundaryReceiptDraft,
};
use meerkat_runtime::{
    MeerkatMachine,
    input::{
        ExternalEventInput, Input, InputDurability, InputHeader, InputOrigin, InputVisibility,
    },
    service_ext::SessionServiceRuntimeExt,
    store::{InMemoryRuntimeStore, RuntimeStore},
};
use std::time::Duration;

pub(super) fn uncaptured_live() -> Arc<LiveSharedSetup> {
    let live = unbound_live();
    live.bind_consent_owner(None).unwrap();
    live
}

pub(super) fn unbound_live() -> Arc<LiveSharedSetup> {
    use meerkat_core::{
        CompiledApplicationToolPolicy, CompiledMemberToolGrants, CompiledPolicySourceProvenance,
        PolicyDigest, PolicyId, PolicyProviderId, PolicyRevision,
    };
    let bytes = CompiledApplicationToolPolicy::new(
        PolicyProviderId::new("application").unwrap(),
        PolicyId::new("selected").unwrap(),
        PolicyRevision(1),
        CompiledPolicySourceProvenance {
            source_id: "selected".into(),
            source_digest: PolicyDigest::from_canonical_bytes(b"selected"),
        },
        vec![CompiledMemberToolGrants {
            member_identity: "ordinary".into(),
            grants: vec![],
        }],
    )
    .unwrap()
    .canonical_json()
    .unwrap();
    LiveSharedSetup::new(
        RealmLocator {
            state_root: PathBuf::from("/unused-census-fixture"),
            realm: meerkat_core::mob_realm_id("application").unwrap(),
        },
        PreparedInstalledToolPolicy::from_canonical(&bytes).unwrap(),
        serde_json::from_value(serde_json::json!({"id":"application","profiles":{}})).unwrap(),
        &[],
    )
}

// Actual native service and Meta forwarding layers over this exact runtime
// store. No session actor or provider is created by this census fixture.
fn spec(
    machine: Arc<MeerkatMachine>,
    store: Arc<dyn RuntimeStore>,
) -> meerkat_mobkit::MobBootstrapSpec {
    spec_with_factory(machine, store, meerkat::AgentFactory::minimal())
}

fn spec_with_factory(
    machine: Arc<MeerkatMachine>,
    store: Arc<dyn RuntimeStore>,
    factory: meerkat::AgentFactory,
) -> meerkat_mobkit::MobBootstrapSpec {
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default()),
        1,
        Arc::new(meerkat_store::MemoryStore::new()),
        store,
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ));
    meerkat_mobkit::MobBootstrapSpec::new(
        meerkat_mob::MobDefinition::explicit(meerkat_mob::MobId::from("native-pair")),
        meerkat_mob::MobStorage::in_memory(),
        service,
    )
    .with_session_runtime_adapter(machine)
    .unwrap()
}

fn live(machine: Arc<MeerkatMachine>, store: Arc<dyn RuntimeStore>) -> Arc<LiveSharedSetup> {
    let selected = uncaptured_live();
    selected.capture_native(&spec(machine, store)).unwrap();
    selected
}

#[tokio::test]
async fn native_context_retains_exact_forwarded_service_and_canonical_machine() {
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let machine = Arc::new(MeerkatMachine::persistent_without_blobs(store.clone()).unwrap());
    let inner = spec(machine.clone(), store);
    // The public Meta constructor adds an actual forwarding layer. A wrapper
    // pointer cannot be used as a canonical native owner identity.
    let mut outer = meerkat_mobkit::MobBootstrapSpec::new(
        inner.definition.clone(),
        meerkat_mob::MobStorage::in_memory(),
        inner.session_service.clone(),
    );
    assert!(!Arc::ptr_eq(&inner.session_service, &outer.session_service));
    assert!(outer.runtime_adapter.is_none());
    let selected = uncaptured_live();
    selected.capture_native(&outer).unwrap();
    let retained = selected.native_context().unwrap();
    assert!(Arc::ptr_eq(&retained.service, &outer.session_service));
    assert!(Arc::ptr_eq(retained.machine().unwrap(), &machine));
    assert!(Arc::ptr_eq(
        &retained
            .service
            .acquire_runtime_adapter(None)
            .unwrap()
            .unwrap(),
        &machine
    ));
    drop(retained);
    let weak_service = Arc::downgrade(&outer.session_service);
    let weak_machine = Arc::downgrade(&machine);
    // Neither a caller replacing its spec field nor dropping the bootstrap
    // owners changes the pair which the live shared owner retained.
    outer.session_service = inner.session_service.clone();
    drop(outer);
    drop(inner);
    drop(machine);
    assert!(weak_service.upgrade().is_some());
    assert!(Arc::ptr_eq(
        selected.native_context().unwrap().machine().unwrap(),
        &weak_machine.upgrade().unwrap()
    ));
    drop(selected);
    assert!(weak_service.upgrade().is_none());
    assert!(weak_machine.upgrade().is_none());
}

#[tokio::test]
async fn native_context_repeated_capture_cannot_replace_either_owner() {
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let machine = Arc::new(MeerkatMachine::persistent_without_blobs(store.clone()).unwrap());
    let first = spec(machine.clone(), store);
    let same_machine = meerkat_mobkit::MobBootstrapSpec::new(
        first.definition.clone(),
        meerkat_mob::MobStorage::in_memory(),
        first.session_service.clone(),
    );
    let other_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let other = spec(
        Arc::new(MeerkatMachine::persistent_without_blobs(other_store.clone()).unwrap()),
        other_store,
    );
    let selected = uncaptured_live();
    selected.capture_native(&first).unwrap();
    for attempted in [&first, &same_machine, &other] {
        assert_eq!(
            selected.capture_native(attempted),
            Err(HostLifetimeError::Denied)
        );
        let retained = selected.native_context().unwrap();
        assert!(Arc::ptr_eq(&retained.service, &first.session_service));
        assert!(Arc::ptr_eq(retained.machine().unwrap(), &machine));
    }
}

#[tokio::test]
async fn native_context_adapter_refusal_preserves_empty_capture_slot() {
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let machine = Arc::new(MeerkatMachine::persistent_without_blobs(store.clone()).unwrap());
    let mut offered = spec(machine.clone(), store.clone());
    // Same store alone is not execution-owner authority. The real service
    // refuses a different machine before Connect can publish either field.
    offered.runtime_adapter = Some(Arc::new(
        MeerkatMachine::persistent_without_blobs(store).unwrap(),
    ));
    let selected = uncaptured_live();
    assert_eq!(
        selected.capture_native(&offered),
        Err(HostLifetimeError::Unavailable)
    );
    assert!(matches!(
        selected.native_context(),
        Err(HostLifetimeError::Unavailable)
    ));
    offered.runtime_adapter = Some(machine.clone());
    selected.capture_native(&offered).unwrap();
    assert!(Arc::ptr_eq(
        &selected.native_context().unwrap().service,
        &offered.session_service
    ));
    assert!(Arc::ptr_eq(
        selected.native_context().unwrap().machine().unwrap(),
        &machine
    ));
}

// This delegate issues no authorization. The actual LiveCallContext installed
// by selected.provider is the same factory field used by gateway composition.
struct UnusedContext;
#[async_trait]
impl meerkat_mcp::McpCallContextProvider for UnusedContext {
    async fn prepare(
        &self,
        _: McpCallTarget<'_>,
        _: ToolCallView<'_>,
        _: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        panic!("this owner-lifetime control must not dispatch a tool")
    }
}

fn configured_native(
    selected: &Arc<LiveSharedSetup>,
) -> (meerkat_mobkit::MobBootstrapSpec, Arc<MeerkatMachine>) {
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let machine = Arc::new(MeerkatMachine::persistent_without_blobs(store.clone()).unwrap());
    let provider = selected.provider(Arc::new(UnusedContext)).unwrap();
    let factory = meerkat::AgentFactory::minimal().mcp_call_context_provider(provider);
    (spec_with_factory(machine.clone(), store, factory), machine)
}

#[tokio::test]
async fn native_context_configured_factory_releases_after_joined_close() {
    let selected = uncaptured_live();
    let weak_live = Arc::downgrade(&selected);
    let (offered, machine) = configured_native(&selected);
    let weak_service = Arc::downgrade(&offered.session_service);
    selected.capture_native(&offered).unwrap();
    drop(offered);
    drop(machine);
    assert!(weak_service.upgrade().is_some());
    // There were no registrations, actors or physical entries. This is the
    // actual Connect joined-close path over an inert configured composition.
    selected.join_after_native_close(true).await.unwrap();
    assert!(matches!(
        selected.native_context(),
        Err(HostLifetimeError::Denied)
    ));
    assert!(
        weak_service.upgrade().is_none(),
        "the factory's live-owner cycle was broken"
    );
    selected.join_after_native_close(true).await.unwrap();
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let rejected = spec(
        Arc::new(MeerkatMachine::persistent_without_blobs(store.clone()).unwrap()),
        store,
    );
    assert_eq!(
        selected.capture_native(&rejected),
        Err(HostLifetimeError::Denied)
    );
    drop(selected);
    assert!(weak_live.upgrade().is_none());
}

#[tokio::test]
async fn native_context_unpolled_close_future_retires_configured_cycle() {
    let selected = uncaptured_live();
    let weak_live = Arc::downgrade(&selected);
    let (offered, machine) = configured_native(&selected);
    let weak_service = Arc::downgrade(&offered.session_service);
    selected.capture_native(&offered).unwrap();
    drop(offered);
    drop(machine);
    selected.begin_close();
    // This is the exact production future passed to tokio::spawn. It is
    // intentionally never polled, so no scheduler timing assumption is used.
    let unpolled = selected.close_task(false);
    assert!(selected.native_context().is_ok());
    assert!(weak_service.upgrade().is_some());
    drop(unpolled);
    assert!(matches!(
        selected.native_context(),
        Err(HostLifetimeError::Denied)
    ));
    assert!(weak_service.upgrade().is_none());
    assert!(
        selected.close_result.get().is_none(),
        "reference release without close execution creates no successful result"
    );
    drop(selected);
    assert!(weak_live.upgrade().is_none());
}

#[tokio::test]
async fn native_context_failed_join_retires_cycle_after_cancelled_observer() {
    use futures_util::FutureExt;
    let selected = uncaptured_live();
    let weak_live = Arc::downgrade(&selected);
    let (offered, machine) = configured_native(&selected);
    let weak_service = Arc::downgrade(&offered.session_service);
    selected.capture_native(&offered).unwrap();
    drop(offered);
    drop(machine);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let task = {
        let entered = entered.clone();
        let release = release.clone();
        tokio::spawn(async move {
            entered.notify_one();
            release.notified().await;
            Err(HostLifetimeError::NotSettled)
        })
    };
    let recovery_id = toolkit_package::Id::new("retained-refusal").unwrap();
    let recovery = Arc::new(tokio::sync::Mutex::new(RecoveryTask {
        task: Some(task),
        result: None,
    }));
    selected
        .recoveries
        .lock()
        .unwrap()
        .insert(recovery_id.clone(), recovery.clone());
    let body = std::panic::AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let observer = selected.join_after_native_close(false);
            tokio::pin!(observer);
            tokio::select! {
                biased;
                result = &mut observer => panic!("close completed before retained recovery: {result:?}"),
                () = entered.notified() => (),
            }
            // Dropping only this observer cannot release service custody.
        }).await.expect("retained close observer entered");
        assert!(selected.native_context().is_ok());
        assert!(weak_service.upgrade().is_some());
    }).catch_unwind().await;
    // Always unblock and join the real retained task, including timeout or
    // assertion failure. Its refusal remains a refusal after pair retirement.
    release.notify_one();
    let closed = selected.join_after_native_close(false).await;
    assert_eq!(closed, Err(HostLifetimeError::NotSettled));
    assert_eq!(selected.join_after_native_close(false).await, closed);
    assert!(
        Arc::ptr_eq(
            selected
                .recoveries
                .lock()
                .unwrap()
                .get(&recovery_id)
                .unwrap(),
            &recovery
        ),
        "retiring a service cycle does not remove unresolved recovery ownership"
    );
    assert_eq!(
        recovery.lock().await.join().await,
        closed,
        "retained failure never becomes a successful resource disposition"
    );
    assert!(matches!(
        selected.native_context(),
        Err(HostLifetimeError::Denied)
    ));
    assert!(weak_service.upgrade().is_none());
    drop(selected);
    assert!(weak_live.upgrade().is_none());
    if let Err(panic) = body {
        std::panic::resume_unwind(panic);
    }
}

fn input() -> Input {
    Input::ExternalEvent(ExternalEventInput {
        objective_id: None,
        header: InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: InputId::new(),
            timestamp: "2026-10-23T00:00:00Z".parse().unwrap(),
            source: InputOrigin::External {
                source_name: "closed-cohort-control".into(),
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        event_type: "closed-cohort-control".into(),
        payload: serde_json::json!({}),
        blocks: None,
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata: None,
    })
}

struct CompletedExecutor(SessionId, bool);
#[async_trait]
impl CoreExecutor for CompletedExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let receipt = RunBoundaryReceiptDraft {
            run_id,
            boundary: RunApplyBoundary::RunStart,
            contributing_input_ids: primitive.contributing_input_ids().to_vec(),
            conversation_digest: None,
            message_count: 0,
        };
        if self.1 {
            return Ok(CoreApplyOutput::with_callback_pending(
                receipt,
                None,
                "closed-cohort-callback",
                "controlled-callback",
                serde_json::json!({"marker":"callback-body"}),
            ));
        }
        Ok(CoreApplyOutput::with_run_result(
            receipt,
            None,
            serde_json::from_value(serde_json::json!({
                "text":"completed", "session_id":self.0, "usage":meerkat_core::Usage::default(),
                "turns":1, "tool_calls":0,
            }))
            .unwrap(),
        ))
    }
    async fn cancel_after_boundary(&mut self, _: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
    async fn stop_runtime_executor(&mut self, _: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

#[tokio::test]
async fn closed_cohort_requires_real_join_and_preserves_pending_baseline() {
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let machine = Arc::new(MeerkatMachine::persistent_without_blobs(store.clone()).unwrap());
    let live = live(machine.clone(), store.clone());
    let session = SessionId::new();
    machine.register_session(session.clone()).await.unwrap();
    assert_eq!(
        live.closed_inputs(&session, &HashSet::new()).await,
        Err(HostLifetimeError::NotSettled),
        "empty live memory is not a joined cohort"
    );
    machine.unregister_session(&session).await.unwrap();
    assert_eq!(
        live.closed_inputs(&session, &HashSet::new()).await.unwrap(),
        Some(HashSet::new())
    );
    let unknown = SessionId::new();
    assert!(live.closed_inputs(&unknown, &HashSet::new()).await.is_err());

    machine.register_session(session.clone()).await.unwrap();
    let queued = input();
    let queued_id = queued.id().clone();
    assert!(
        machine
            .accept_input(&session, queued)
            .await
            .unwrap()
            .is_accepted()
    );
    // A fresh machine can read the exact persistent rows but cannot claim a
    // queued input settled merely because it has no local registration.
    let detached = live_for_store(store);
    assert_eq!(
        detached
            .closed_inputs(&session, &HashSet::from([queued_id.clone()]))
            .await
            .unwrap(),
        None
    );
    machine.unregister_session(&session).await.unwrap();
    assert_eq!(
        live.closed_inputs(&session, &HashSet::from([queued_id]))
            .await
            .unwrap(),
        None,
        "shutdown must not turn earlier pending work into reusable closure"
    );
}

fn live_for_store(store: Arc<dyn RuntimeStore>) -> Arc<LiveSharedSetup> {
    live(
        Arc::new(
            MeerkatMachine::persistent(
                store.clone(),
                Arc::new(meerkat_store::MemoryBlobStore::new()),
            )
            .unwrap(),
        ),
        store,
    )
}

#[tokio::test]
async fn closed_cohort_reobserves_actual_completed_receipt_and_exact_baseline() {
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let machine = Arc::new(
        MeerkatMachine::persistent(
            store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .unwrap(),
    );
    let selected = live(machine.clone(), store.clone());
    let session = SessionId::new();
    machine
        .register_session_with_executor(
            session.clone(),
            Box::new(CompletedExecutor(session.clone(), false)),
        )
        .await
        .unwrap();
    let event = input();
    let id = event.id().clone();
    assert!(
        machine
            .accept_input(&session, event)
            .await
            .unwrap()
            .is_accepted()
    );
    let completed = tokio::time::timeout(
        Duration::from_secs(20),
        machine.wait_input_terminal_receipt(&session, &id),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert!(matches!(
        completed,
        meerkat_runtime::terminal_status::InputTerminalReceiptWait::Resolved(_)
    ));
    machine.unregister_session(&session).await.unwrap();
    let observed = machine
        .input_terminal_receipt_observation(&session, InteractionSelector::InputId(id.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.source, TerminalWitnessSource::DurableStore);
    let InputTerminalReceiptObservation::Finalized(receipt) = observed.report else {
        panic!("completed input must retain its actual native receipt");
    };
    let meerkat_runtime::terminal_status::InputTerminalReceiptScope::Run { run_id } =
        receipt.scope()
    else {
        panic!("completed input must retain its actual run-scoped batch");
    };
    assert_eq!(receipt.run_id(), Some(run_id));
    assert_eq!(receipt.class(), CompletionClass::Completed);
    assert_eq!(
        cold::retained_receipt_scope(receipt.scope()).unwrap(),
        serde_json::json!({"type":"run","run_id":run_id})
    );
    let expected = HashSet::from([id]);
    assert_eq!(
        selected
            .closed_inputs(&session, &HashSet::new())
            .await
            .unwrap(),
        Some(expected.clone())
    );
    let restored = live_for_store(store);
    assert_eq!(
        restored.closed_inputs(&session, &expected).await.unwrap(),
        Some(expected)
    );
    assert!(
        restored
            .closed_inputs(&session, &HashSet::from([InputId::new()]))
            .await
            .is_err(),
        "a missing baseline row cannot be filtered out"
    );
    machine.register_session(session.clone()).await.unwrap();
    assert_eq!(
        selected.closed_inputs(&session, &HashSet::new()).await,
        Err(HostLifetimeError::NotSettled)
    );
    machine.unregister_session(&session).await.unwrap();
}

#[tokio::test]
async fn closed_cohort_does_not_close_a_finalized_callback_pending_input() {
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let machine = Arc::new(
        MeerkatMachine::persistent(
            store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .unwrap(),
    );
    let selected = live(machine.clone(), store.clone());
    let session = SessionId::new();
    machine
        .register_session_with_executor(
            session.clone(),
            Box::new(CompletedExecutor(session.clone(), true)),
        )
        .await
        .unwrap();
    let event = input();
    let id = event.id().clone();
    let body = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        assert!(
            machine
                .accept_input(&session, event)
                .await
                .unwrap()
                .is_accepted()
        );
        let completed = tokio::time::timeout(
            Duration::from_secs(20),
            machine.wait_input_terminal_receipt(&session, &id),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert!(matches!(
            completed,
            meerkat_runtime::terminal_status::InputTerminalReceiptWait::Resolved(_)
        ));
        machine.unregister_session(&session).await.unwrap();
        let observed = machine
            .input_terminal_receipt_observation(&session, InteractionSelector::InputId(id.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(observed.source, TerminalWitnessSource::DurableStore);
        let InputTerminalReceiptObservation::Finalized(receipt) = observed.report else {
            panic!("actual callback boundary must have a finalized native receipt");
        };
        assert_eq!(receipt.input_id(), &id);
        assert_eq!(receipt.terminal(), &InputTerminalOutcome::Consumed);
        assert_eq!(receipt.class(), CompletionClass::CallbackPending);
        assert_eq!(receipt.recipient_input_ids(), std::slice::from_ref(&id));
        let baseline = HashSet::from([id]);
        assert_eq!(
            selected.closed_inputs(&session, &baseline).await.unwrap(),
            None,
            "a finalized callback is not completed work"
        );
        assert_eq!(
            live_for_store(store)
                .closed_inputs(&session, &baseline)
                .await
                .unwrap(),
            None,
            "a fresh observer cannot turn a pending callback into closed work"
        );
    }))
    .await;
    if machine
        .current_session_registration_witness(&session)
        .await
        .is_some()
    {
        machine.unregister_session(&session).await.unwrap();
    }
    if let Err(panic) = body {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn losing_restore_observer_retains_join_and_terminal_failure_without_restart() {
    use std::sync::atomic::AtomicUsize;
    let started = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let task = {
        let started = started.clone();
        let entered = entered.clone();
        let release = release.clone();
        tokio::spawn(async move {
            started.fetch_add(1, Ordering::SeqCst);
            entered.notify_one();
            release.notified().await;
            Err(HostLifetimeError::NotSettled)
        })
    };
    let retained = Arc::new(tokio::sync::Mutex::new(RecoveryTask {
        task: Some(task),
        result: None,
    }));
    let observer = {
        let retained = retained.clone();
        tokio::spawn(async move { retained.lock().await.join().await })
    };
    entered.notified().await;
    observer.abort();
    assert!(observer.await.unwrap_err().is_cancelled());
    release.notify_one();
    assert_eq!(
        retained.lock().await.join().await,
        Err(HostLifetimeError::NotSettled)
    );
    assert_eq!(
        retained.lock().await.join().await,
        Err(HostLifetimeError::NotSettled)
    );
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "failed custody is observed again, never restarted"
    );
}

#[test]
fn derived_inputs_require_a_completed_recipient_without_cycles_or_missing_targets() {
    let a = InputId::new();
    let b = InputId::new();
    let completed = InputId::new();
    let settled = HashSet::from([completed.clone()]);
    assert!(derived_inputs_settled(
        &settled,
        &HashMap::from([(a.clone(), b.clone()), (b.clone(), completed.clone())])
    ));
    assert!(!derived_inputs_settled(
        &HashSet::new(),
        &HashMap::from([(a.clone(), completed)])
    ));
    assert!(!derived_inputs_settled(
        &settled,
        &HashMap::from([(a.clone(), b.clone())])
    ));
    assert!(!derived_inputs_settled(
        &settled,
        &HashMap::from([(a.clone(), b.clone()), (b, a)])
    ));
}

#[test]
fn association_selection_preserves_exact_owner_session_and_closed_source_bytes() {
    use meerkat_authorization_contracts::{
        constraints::{AudienceRef, ResourceDomain},
        evidence::EvidenceId,
    };
    use meerkat_core::{MobMemberBinding, PrincipalKind, PrincipalRef, TrustDomainId};
    use meerkat_toolkit::host_lifetime::policy::{
        CompiledMemberClass, CompiledMemberClassInstance, CompiledNativeSourceOrigin,
        CompiledNativeSourceRoute, PreparedNativeMember,
    };
    use toolkit_mobkit::{
        installed_host::dynamic_members::{MemberClassDestination, MemberClassWorkflowBinding},
        private_originals::PrivateOriginalRef,
    };
    use toolkit_package::Id;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let realm = RealmLocator {
        state_root: root.clone(),
        realm: meerkat_core::mob_realm_id("application").unwrap(),
    };
    let artifact = root
        .join(".meerkat-connect")
        .join(realm.realm.as_str())
        .join("selected");
    std::fs::create_dir_all(&artifact).unwrap();
    let handoff = serde_json::to_vec(&serde_json::json!({"mcpServers":{"toolkit-setup":{
        "command":"/owned/fixture-python", "args":[]}}}))
    .unwrap();
    std::fs::write(artifact.join("prepare.stdout"), handoff).unwrap();
    let resource = prepared_resources(&artifact, &realm).unwrap();
    let session = SessionId::new();
    let context = EvidenceId::new(session.to_string()).unwrap();
    let principal = PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        "application",
        TrustDomainId::new("resource-selection-control").unwrap(),
    )
    .unwrap();
    let member = MobMemberBinding {
        mob_id: "application".into(),
        role: "shared".into(),
        member: "shared-g".into(),
    };
    let class = CompiledMemberClass {
        id: Id::new("shared").unwrap(),
        purpose: CompiledMemberClassPurpose::SharedWorkflow,
        profile: "shared".into(),
        factory: Id::new("shared-factory").unwrap(),
        callback_account: None,
        callbacks: vec![],
        grants: vec![],
    };
    let reference = |name: &str| PrivateOriginalRef {
        route: Id::new(name).unwrap(),
        original: EvidenceId::new(name).unwrap(),
        event_sha256: "a".repeat(64),
    };
    // Named provenance fixture, not a fabricated native admission or approval.
    // The canonical CAS and actual native/physical owners are separate tests.
    let admission = MemberClassAdmission {
        standing_operations: None,
        id: Id::new("selected-g").unwrap(),
        instance: CompiledMemberClassInstance {
            class: class.id.clone(),
            member: PreparedNativeMember {
                member: member.clone(),
                logical_owner: principal.clone(),
                context_generation: 1,
                audience: AudienceRef::Principal {
                    principal: principal.clone(),
                },
                setup_capability: None,
            },
        },
        class,
        workflow: Some(MemberClassWorkflowBinding {
            workflow: serde_json::from_value(serde_json::json!({
                "key":{"realm":"realm","owner":"application","id":"workflow"},
                "terms_digest":toolkit_package::ContentDigest::of_bytes(b"workflow") }))
            .unwrap(),
            parent: reference("parent"),
            original: reference("original"),
        }),
        workflow_resource: None,
        destination: Some(MemberClassDestination {
            context: context.clone(),
            context_generation: 1,
        }),
        installed_surface: Some(vec![]),
        native_surface: Some(vec![]),
        route: Some(CompiledNativeSourceRoute {
            id: Id::new("original").unwrap(),
            origin: CompiledNativeSourceOrigin::LocalConsole {
                member,
                requester: principal.clone(),
                ingress_actor: principal.clone(),
            },
            application_parent: None,
            controller_parent: None,
            logical_executor: principal.clone(),
            represented_subject: None,
            source_authority: principal.clone(),
            authentication_domain: ResourceDomain {
                authority: principal.clone(),
                namespace: "login".into(),
            },
            event_domain: ResourceDomain {
                authority: principal,
                namespace: "event".into(),
            },
        }),
        phase: MemberClassAdmissionPhase::Prepared {
            context: context.clone(),
            context_generation: 1,
        },
    };
    let source = artifact.join("approval-source.json");
    std::fs::write(&source, b"retired owner bytes").unwrap();
    let baseline = [
        InputId::from_uuid(uuid::Uuid::from_u128(1)),
        InputId::from_uuid(uuid::Uuid::from_u128(3)),
    ];
    let open = ResourceAssociation {
        schema_version: 1,
        admission: admission.clone(),
        session: session.clone(),
        baseline: HashSet::from(baseline.clone()),
        resources: resource,
        approval_source: source.clone(),
        physical: None,
        consent: None,
        projected_peer: None,
        abandoned_predecessor: None,
        predecessor: None,
        closed: None,
    }
    .retain()
    .unwrap();
    let mut current = admission;
    current.phase = MemberClassAdmissionPhase::Active {
        context,
        context_generation: 1,
    };
    current.workflow_resource = Some(open.selection.clone());
    open.check_open(&current, &realm, &artifact).unwrap();
    check_consent_artifact_linkage(&open, &current, &realm, &root);
    let selected_abandonment = crate::application_consent::AbandonedWorkflowRecovery::new(
        vec![open.selection.clone()],
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(selected_abandonment.selects(&open.selection));
    assert!(
        open.document.projected_peer.is_none()
            && open.document.physical.is_none()
            && open.document.consent.is_none(),
        "selection cannot upgrade legacy missing evidence"
    );
    let mut changed_selection = open.selection.clone();
    changed_selection.sha256 = "b".repeat(64);
    assert!(!selected_abandonment.selects(&changed_selection));
    assert!(
        crate::application_consent::AbandonedWorkflowRecovery::new(
            vec![open.selection.clone(), changed_selection],
            Duration::from_secs(5)
        )
        .is_err(),
        "one path cannot select conflicting immutable associations"
    );
    assert!(
        RetainedAssociation::closed(&current, &realm).is_err(),
        "an open owner cannot be restored"
    );
    let mut foreign = current.clone();
    foreign.id = Id::new("foreign").unwrap();
    assert!(open.check_open(&foreign, &realm, &artifact).is_err());
    let mut changed = current.clone();
    changed.phase = MemberClassAdmissionPhase::Active {
        context: EvidenceId::new(SessionId::new().to_string()).unwrap(),
        context_generation: 1,
    };
    assert!(active_session(&changed).is_err());
    let physical_path = artifact.join("physical-source.json");
    std::fs::write(&physical_path, b"physical owner selection").unwrap();
    let mut document = open.document.clone();
    document.predecessor = Some(open.selection.clone());
    let inputs = [
        baseline[0].clone(),
        InputId::from_uuid(uuid::Uuid::from_u128(2)),
        baseline[1].clone(),
    ];
    document.closed = Some(ClosedObservations {
        physical: SelectedHostArtifact {
            path: physical_path,
            sha256: digest(b"physical owner selection"),
        },
        approvals: SelectedHostArtifact {
            path: source.clone(),
            sha256: digest(b"retired owner bytes"),
        },
        inputs: HashSet::from(inputs.clone()),
        peer: McpServerConfig::stdio(
            "toolkit-setup",
            "/owned/fixture-python",
            vec![],
            Default::default(),
        ),
    });
    let raw = toolkit_package::canonical_json(&document).unwrap();
    let encoded: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(encoded["baseline"], serde_json::json!(baseline));
    assert_eq!(encoded["closed"]["inputs"], serde_json::json!(inputs));
    let mut reordered = document.clone();
    reordered.baseline = baseline.iter().rev().cloned().collect();
    reordered.closed.as_mut().unwrap().inputs = inputs.iter().rev().cloned().collect();
    assert_eq!(
        toolkit_package::canonical_json(&reordered).unwrap(),
        raw,
        "set insertion order cannot change the retained association digest"
    );
    let decoded: ResourceAssociation = serde_json::from_slice(&raw).unwrap();
    assert_eq!(decoded.baseline, document.baseline);
    assert_eq!(
        decoded.closed.as_ref().unwrap().inputs,
        document.closed.as_ref().unwrap().inputs
    );
    assert_eq!(
        ordered_input_ids(&decoded.closed.as_ref().unwrap().inputs),
        inputs.iter().collect::<Vec<_>>()
    );
    let closed = document.retain().unwrap();
    current.workflow_resource = Some(closed.selection.clone());
    assert!(
        open.check_open(&current, &realm, &artifact).is_err(),
        "canonical successor rejects stale in-memory selection"
    );
    RetainedAssociation::closed(&current, &realm).unwrap();
    assert!(
        !selected_abandonment.selects(&closed.selection),
        "explicit old selection cannot redirect a successor to abandonment"
    );
    let mut missing = closed.document.clone();
    missing.closed.as_mut().unwrap().inputs.remove(&baseline[0]);
    let mut missing_current = current.clone();
    missing_current.workflow_resource = Some(missing.retain().unwrap().selection);
    assert!(
        RetainedAssociation::closed(&missing_current, &realm).is_err(),
        "a closed selection cannot omit an original baseline input"
    );
    std::fs::write(&source, b"later unmatched decision").unwrap();
    assert!(
        RetainedAssociation::closed(&current, &realm).is_err(),
        "changed prior decisions cannot be silently reused"
    );
    std::fs::write(&source, b"retired owner bytes").unwrap();
    std::fs::write(&closed.selection.path, b"{}").unwrap();
    assert!(
        RetainedAssociation::closed(&current, &realm).is_err(),
        "selected association bytes remain integrity checked"
    );
}

fn consent_owner(root: &Path) -> Arc<crate::application_consent::ApplicationConsentOwner> {
    use toolkit_mobkit::installed_host::{
        PreparedInstallationOwner, operator_setup::OperatorOwnerStartup,
    };
    let (_, mut selected) =
        crate::application_setup::compilation::tests::helperless_input(root, true);
    selected.native_host.remote_operator = Some(
        meerkat_core::PrincipalRef::in_domain(
            meerkat_core::PrincipalKind::ServiceAccount,
            "consent-provenance-operator",
            meerkat_core::TrustDomainId::new("application-setup-fixture").unwrap(),
        )
        .unwrap(),
    );
    std::fs::create_dir_all(root.join("shared/data")).unwrap();
    let prepared = PreparedInstallationOwner::open(
        root.to_owned(),
        selected,
        OperatorOwnerStartup::fresh("consent-resource-link", 1).unwrap(),
    )
    .unwrap();
    crate::application_consent::ApplicationConsentOwner::create(
        root,
        crate::application_consent::ConsentApplicationIdentity::from_preparation(&prepared)
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn consent_owner_selection_is_frozen_before_native_capture() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let issuer = consent_owner(&root);
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let machine = Arc::new(MeerkatMachine::persistent_without_blobs(store.clone()).unwrap());
    let offered = spec(machine.clone(), store);
    let selected = unbound_live();
    assert_eq!(
        selected.capture_native(&offered),
        Err(HostLifetimeError::Unavailable),
        "capture cannot silently default a missing composition selection"
    );
    selected.bind_consent_owner(Some(issuer.clone())).unwrap();
    assert_eq!(
        selected.bind_consent_owner(None),
        Err(HostLifetimeError::Denied)
    );
    assert_eq!(
        selected.bind_consent_owner(Some(issuer.clone())),
        Err(HostLifetimeError::Denied)
    );
    selected.capture_native(&offered).unwrap();
    assert!(Arc::ptr_eq(
        selected.consent_owner.get().unwrap().as_ref().unwrap(),
        &issuer
    ));
    assert_eq!(
        selected.bind_consent_owner(Some(issuer.clone())),
        Err(HostLifetimeError::Denied)
    );
    let ordinary = uncaptured_live();
    ordinary.capture_native(&offered).unwrap();
    assert!(ordinary.consent_owner.get().unwrap().is_none());
    assert_eq!(
        ordinary.bind_consent_owner(Some(issuer)),
        Err(HostLifetimeError::Denied),
        "ordinary None cannot be upgraded after a factory has captured native ownership"
    );
    selected.join_after_native_close(true).await.unwrap();
    ordinary.join_after_native_close(true).await.unwrap();
}

// Reuse the actual immutable association writer/reader above. The admission is
// a named serialization fixture, not claimed canonical/native custody. Claims
// below are real owner-issued OS claims; no actor, consent or restoration runs.
fn check_consent_artifact_linkage(
    open: &RetainedAssociation,
    current: &MemberClassAdmission,
    realm: &RealmLocator,
    root: &Path,
) {
    use meerkat_core::session_hosting::HostingPaths;
    let issuer_root = root.join("issuer-fixture");
    std::fs::create_dir(&issuer_root).unwrap();
    let issuer = consent_owner(&issuer_root);
    let native_root = root.join("native-provenance");
    std::fs::create_dir_all(native_root.join("hosting")).unwrap();
    std::fs::create_dir_all(native_root.join("delivery")).unwrap();
    let store: Arc<dyn RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(
            native_root.join("runtime.sqlite3"),
        )
        .unwrap()
        .with_hosting_paths(HostingPaths {
            hosting_lock_dir: native_root.join("hosting"),
            cold_delivery_lock: native_root.join("delivery/cold.lock"),
            database: None,
        }),
    );
    let machine = Arc::new(MeerkatMachine::persistent_without_blobs(store.clone()).unwrap());
    let claim = machine
        .grant_session_hosting(&open.document.session)
        .unwrap();
    assert!(claim.is_cross_process());
    let ordinary = live(machine.clone(), store.clone());
    ordinary.check_consent_provenance(&open.document).unwrap();
    assert!(
        matches!(
            RetainedAssociation::bind_consent(current, realm, &issuer, &machine, &claim),
            Err(HostLifetimeError::NotSettled)
        ),
        "ordinary/legacy bytes are not an inferred issuer"
    );
    let original_bytes = read_selected(&open.selection).unwrap();
    assert!(
        serde_json::from_slice::<serde_json::Value>(&original_bytes)
            .unwrap()
            .get("consent")
            .is_none()
    );
    let selected = unbound_live();
    selected.bind_consent_owner(Some(issuer.clone())).unwrap();
    selected
        .capture_native(&spec(machine.clone(), store.clone()))
        .unwrap();
    assert_eq!(
        selected.check_consent_provenance(&open.document),
        Err(HostLifetimeError::NotSettled),
        "a newly selected issuer cannot upgrade existing Unknown history"
    );
    let mut document = open.document.clone();
    document.consent = Some(
        issuer
            .resource_provenance(&machine, &document.session)
            .unwrap(),
    );
    let linked = document.retain().unwrap();
    let mut linked_current = current.clone();
    linked_current.workflow_resource = Some(linked.selection.clone());
    selected.check_consent_provenance(&linked.document).unwrap();
    assert_eq!(
        ordinary.check_consent_provenance(&linked.document),
        Err(HostLifetimeError::NotSettled)
    );
    let binding =
        RetainedAssociation::bind_consent(&linked_current, realm, &issuer, &machine, &claim)
            .unwrap();
    assert_eq!(binding.artifact(), &linked.selection);
    assert_eq!(binding.scope().session_id, linked.document.session);
    assert_eq!(
        Some(&binding.scope().domain),
        linked.document.consent.as_ref().unwrap().domain()
    );
    assert_ne!(binding.artifact(), &open.selection);
    assert_eq!(
        read_selected(&open.selection).unwrap(),
        original_bytes,
        "the old Unknown artifact remains exact"
    );
    let second = MeerkatMachine::persistent_without_blobs(store).unwrap();
    assert!(
        matches!(
            RetainedAssociation::bind_consent(&linked_current, realm, &issuer, &second, &claim),
            Err(HostLifetimeError::WrongOwner)
        ),
        "same domain does not make another machine the claim owner"
    );
    let other_session = SessionId::new();
    let other_claim = machine.grant_session_hosting(&other_session).unwrap();
    assert!(matches!(
        RetainedAssociation::bind_consent(&linked_current, realm, &issuer, &machine, &other_claim),
        Err(HostLifetimeError::WrongOwner)
    ));
    let local_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let local_machine =
        Arc::new(MeerkatMachine::persistent_without_blobs(local_store.clone()).unwrap());
    let local_live = unbound_live();
    local_live.bind_consent_owner(Some(issuer.clone())).unwrap();
    local_live
        .capture_native(&spec(local_machine.clone(), local_store))
        .unwrap();
    let mut local_doc = open.document.clone();
    local_doc.consent = Some(
        issuer
            .resource_provenance(&local_machine, &local_doc.session)
            .unwrap(),
    );
    local_live.check_consent_provenance(&local_doc).unwrap();
    let local = local_doc.retain().unwrap();
    let mut local_current = current.clone();
    local_current.workflow_resource = Some(local.selection);
    let local_claim = local_machine
        .grant_session_hosting(&local.document.session)
        .unwrap();
    assert!(
        matches!(
            RetainedAssociation::bind_consent(
                &local_current,
                realm,
                &issuer,
                &local_machine,
                &local_claim
            ),
            Err(HostLifetimeError::WrongOwner)
        ),
        "selected process-local history is explicit but not recoverable"
    );
    let retained_bytes = read_selected(&linked.selection).unwrap();
    std::fs::write(&linked.selection.path, b"{}").unwrap();
    assert!(
        RetainedAssociation::bind_consent(&linked_current, realm, &issuer, &machine, &claim)
            .is_err()
    );
    std::fs::write(&linked.selection.path, retained_bytes).unwrap();
    // Inert service/factory fixtures have no callback retaining these owners.
    // The existing configured-cycle controls cover joined close separately.
    drop(local_live);
    drop(selected);
    drop(ordinary);
}

#[tokio::test]
async fn abandoned_selection_freezes_before_capture_without_assuming_unbound_capacity() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let owner = consent_owner(&root);
    let raw = b"inert selected association bytes";
    let path = root.join("abandoned-selection.json");
    std::fs::write(&path, raw).unwrap();
    let selected = crate::application_consent::AbandonedWorkflowRecovery::new(
        vec![SelectedHostArtifact {
            path,
            sha256: digest(raw),
        }],
        Duration::from_secs(5),
    )
    .unwrap();
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let machine = Arc::new(MeerkatMachine::persistent_without_blobs(store.clone()).unwrap());
    let offered = spec(machine, store);
    let linked = unbound_live();
    assert_eq!(
        linked.policy.member_class_registration_capacity().unwrap(),
        0,
        "this actual pre-composition policy owner is not bound yet"
    );
    linked.bind_consent_owner(Some(owner)).unwrap();
    linked
        .bind_abandoned_recovery(Some(selected.clone()))
        .unwrap();
    assert_eq!(
        linked.bind_abandoned_recovery(None),
        Err(HostLifetimeError::Denied)
    );
    linked.capture_native(&offered).unwrap();
    assert_eq!(
        linked.bind_abandoned_recovery(Some(selected.clone())),
        Err(HostLifetimeError::Denied)
    );
    let ordinary = uncaptured_live();
    assert_eq!(
        ordinary.bind_abandoned_recovery(Some(selected)),
        Err(HostLifetimeError::NotSettled),
        "selection cannot manufacture an absent consent owner"
    );
    ordinary.bind_abandoned_recovery(None).unwrap();
    ordinary.capture_native(&offered).unwrap();
    assert!(ordinary.abandoned.get().unwrap().is_none());
    assert!(ordinary.cold_recoveries.lock().unwrap().is_empty());
}

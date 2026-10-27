//! Intact-pair cold census controls. A real retained event is still unbound;
//! no native input, terminal admission, or delivered wake is manufactured.
use super::*;
use meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy;

async fn open_pair(selection: &HostBootstrapSelection, local: &CompiledLocalPolicy,
    capture: Arc<OnceLock<Arc<PausedSourceStore>>>, scope: StoreScope,
) -> PreparedInstalledHost {
    let selected = local.native_host.as_ref().unwrap();
    let policy = PreparedInstalledToolPolicy::from_canonical(
        &read_bundle_artifact(selection, &selected.application_tool_policy).unwrap()).unwrap();
    PreparedInstallationOwner::open(selection.deployment_root.clone(), PreparedLocalPolicy {
        deployment: local.deployment.clone(), components: local.components.clone(),
        channels: local.channels.clone(), native_host: selected.preparation(),
    }, operator_setup::OperatorOwnerStartup::Restore).unwrap()
        .complete_with_source_store(policy, Box::new(move |inner| {
            let store = Arc::new(PausedSourceStore::new(inner, scope));
            assert!(capture.set(store.clone()).is_ok());
            store
        })).await.unwrap()
}

#[tokio::test]
async fn intact_pair_reopen_joins_unknown_selected_version_unavailable_without_retry() {
    let (_directory, mut selection) = fixture();
    provision_fixture(&mut selection);
    let local: CompiledLocalPolicy = serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    let selected = local.native_host.as_ref().unwrap();
    let state = state_directory(&selection, selected, NativeOwnerStartup::Restore).unwrap();
    let grants = fs::read(state.join("grants.jsonl")).unwrap();
    let scope = StoreScope { realm: selected.realm.to_string(), owner: local.deployment.to_string() };
    let valid = {
        let capture = Arc::new(OnceLock::new());
        let prepared = open_pair(&selection, &local, capture.clone(), scope.clone()).await;
        let source = prepared.originals();
        let policy_rows = capture.get().unwrap().list_records(&scope, None, 32).await.unwrap();
        assert_eq!(policy_rows.len(), 1);
        assert_eq!(policy_rows[0].data.schema, "toolkit.application-service-tool-policy");
        let mut event = crate::original_ingress::tests::private_event("cold-version");
        event.route = toolkit_package::Id::new("console").unwrap();
        let expected_event = serde_json::to_value(&event).unwrap();
        source.retain_event(event).await.unwrap();
        let rows = capture.get().unwrap().list_records(&scope, None, 32).await.unwrap();
        assert_eq!(rows.len(), policy_rows.len() + 1);
        assert_eq!(capture.get().unwrap().get(&policy_rows[0].key).await.unwrap().unwrap(), policy_rows[0]);
        let mut events = rows.into_iter().filter(|row|
            row.data.schema == "toolkit.authenticated-source-event");
        let retained = events.next().expect("the actual retained source event");
        assert!(events.next().is_none());
        assert_eq!(retained.key.realm, scope.realm);
        assert_eq!(retained.key.owner, scope.owner);
        assert_eq!(retained.data.body["event"], expected_event);
        assert_eq!(retained.data.version, 3);
        assert!(retained.data.body.get("custody_attempt").is_none());
        retained
        // Every owner of this exact pair is dropped before actual reopening.
    };
    let capture = Arc::new(OnceLock::new());
    let prepared = open_pair(&selection, &local, capture.clone(), scope.clone()).await;
    let originals = prepared.originals();
    let store = capture.get().unwrap().clone();
    assert_eq!(store.get(&valid.key).await.unwrap().unwrap(), valid);
    let policy = PreparedInstalledToolPolicy::from_canonical(
        &read_bundle_artifact(&selection, &selected.application_tool_policy).unwrap()).unwrap();
    let registration = Arc::new(prepared_registration::PreparedMemberRegistration::new(
        Arc::new(UnenteredPreparationRegistration), policy.registry(),
        Arc::new(|_, _| panic!("cold census must not select a native destination")),
    ));
    let participant = prepared.gateway_runtime(registration,
        acquisition::InstalledAcquisitionTools::new(Duration::from_secs(10)).unwrap(),
        [toolkit_package::Id::new("hook-fixture-viewer").unwrap()].into()).unwrap();
    // HostJobSource requires the same real runtime store/jobs as this service.
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        Arc::new(meerkat::SqliteSessionStore::open(state.join("prepared-native-sessions.sqlite3")).unwrap()),
        runtime_store.clone(), Arc::new(meerkat::MemoryBlobStore::new()),
        prepared.take_configuration().unwrap(),
    ).unwrap();
    let jobs = persistence.job_store();
    let (service, machine) = meerkat::surface::build_runtime_backed_service(
        FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default()),
        1, persistence,
    ).unwrap();
    let service = Arc::new(service);
    let spec = MobBootstrapSpec::new(definition(), MobStorage::in_memory(), service.clone())
        .with_session_runtime_adapter(machine).unwrap().with_declared_ephemeral_mob_storage()
        .with_runtime_delivery(meerkat_mobkit::mob_handle_runtime::MobRuntimeDelivery::new(runtime_store, jobs));
    let spec = participant.prepare_bootstrap(spec).unwrap();
    let mut native = UnifiedRuntime::bootstrap(spec, MetaConfig {
        modules: Vec::new(), discovery: DiscoverySpec { namespace: "factory".into(), modules: Vec::new() },
        pre_spawn: Vec::new(),
    }, Duration::from_secs(10)).await.unwrap();
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(20), async {
        let mut unsupported = valid.data.clone();
        unsupported.version += 10;
        let unsupported = store.compare_and_swap(Transition {
            key: valid.key.clone(), expected_revision: valid.revision,
            expected_digest: Some(valid.content_digest.clone()), claim: None,
            data: unsupported, events: vec![], effects: vec![],
        }).await.unwrap();
        native.install_identity_first_context(context(&native, true));
        let reads_before = store.reads.load(Ordering::SeqCst);
        store.arm();
        participant.bind_identity_runtime(&mut native).unwrap();
        store.entered.notified().await;
        assert_eq!(store.started.load(Ordering::SeqCst), 1);
        assert_eq!(store.completed.load(Ordering::SeqCst), 0);
        assert_eq!(store.reads.load(Ordering::SeqCst), reads_before + 1);
        participant.begin_custody_close();
        let mut join = Box::pin(participant.join_custody_close());
        assert!(matches!(futures::poll!(join.as_mut()), std::task::Poll::Pending),
            "stop cannot abandon the actual entered census read");
        assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
        store.release.add_permits(1);
        assert!(join.await.is_err(), "unknown selected data cannot be a clean source close");
        assert_eq!(store.completed.load(Ordering::SeqCst), 1);
        assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
        assert_eq!(store.get(&valid.key).await.unwrap().unwrap(), unsupported);
        assert!(participant.join_custody_close().await.is_err(),
            "repeated join retains the same completed Unavailable result");
        assert_eq!(store.reads.load(Ordering::SeqCst), reads_before + 1,
            "a completed failed close must not schedule another census read");
        assert!(service.list(SessionQuery::default()).await.unwrap().is_empty());
        assert_eq!(fs::read(state.join("grants.jsonl")).unwrap(), grants);
        assert!(originals.start_custody_recovery(None).is_err());
    })).catch_unwind().await;
    // Keep the controlled unsupported row intact. Release and join the exact
    // entered read and native owners even when the body observer failed.
    participant.begin_custody_close();
    store.release.add_permits(1);
    let native_cleanup = native.shutdown().await;
    let source_cleanup = participant.join_custody_close().await;
    assert!(native_cleanup.cleanup_completed());
    assert!(match native_cleanup.mob_terminal_shutdown {
        MobTerminalShutdownOutcome::Completed(report) => report.is_clean(),
        MobTerminalShutdownOutcome::AlreadyShutDown => true,
        _ => false,
    });
    assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
    match checked {
        Ok(Ok(())) => assert!(source_cleanup.is_err(), "completed failure remains Unavailable"),
        Ok(Err(_)) => panic!("bounded intact-pair version refusal control"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test]
async fn generic_canonical_adapter_does_not_claim_installed_pair_cold_eligibility() {
    let directory = tempfile::tempdir().unwrap();
    let owner = crate::original_ingress::tests::initialized(&directory.path().join("grants"));
    let scope = StoreScope { realm: "test-realm".into(), owner: "originals".into() };
    let store = Arc::new(PausedSourceStore::new(Arc::new(
        toolkit_store::SqliteStore::open(directory.path().join("originals.sqlite3")).unwrap()), scope));
    let source = crate::original_ingress::tests::private_originals_with_store(owner.clone(), store.clone()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(10), async {
        assert!(!owner.has_installed_pair_recovery().unwrap());
        store.arm();
        source.start_custody_recovery(None).unwrap();
        source.stop_custody_recovery();
        source.join_custody_recovery().await.unwrap();
        assert_eq!(store.started.load(Ordering::SeqCst), 0);
        assert_eq!(store.completed.load(Ordering::SeqCst), 0);
        assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
    })).catch_unwind().await;
    source.stop_custody_recovery();
    store.release.add_permits(1);
    source.join_custody_recovery().await.unwrap();
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("generic adapter attempted installed-pair census"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

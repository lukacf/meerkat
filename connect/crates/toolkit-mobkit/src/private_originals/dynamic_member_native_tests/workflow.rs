//! Real class construction and source isolation. This does not submit G's
//! independent planning input or claim public setup/MCP acceptance.
use super::unaccepted_retirement::CleanupProbe;
use super::*;
use meerkat_core::{SessionServiceHistoryExt, service::SessionHistoryQuery};
use meerkat_mobkit::identity_first::AgentAddressability;
use meerkat_toolkit::{
    host_lifetime::policy::CompiledMemberClassPurpose,
    setup::{SetupWorkBackend, SetupWorkflowTerms},
};
use meerkat_workgraph::{
    ClaimWorkItemRequest, CreateWorkItemRequest, SqliteWorkGraphStore, WorkGraphService,
    WorkItemRef, WorkNamespace, WorkOwner, WorkOwnerKey,
};
use toolkit_store::*;

pub(super) async fn parent(fixture: &Fixture) -> PrivateOriginalRef {
    let runtime = fixture.source.bound_identity_runtime().unwrap();
    let identity = AgentIdentity::parse(MEMBER).unwrap();
    let status = runtime.status(&identity).await.unwrap();
    assert_eq!(status.state, IdentityLifecycleState::Active);
    let session = status.session_id.expect("actual parent session required");
    // The restored parent is seated but has no live actor until its owner
    // materializes it. Inspect the controller before submitting any input.
    let requests_before = fixture.provider.0.load(Ordering::SeqCst);
    let materialized = runtime
        .materialize_tracked(&identity)
        .await
        .unwrap()
        .expect("the workflow parent requires a session-backed member");
    assert_eq!(
        materialized.session_id(),
        &session,
        "materialization must preserve the restored parent session"
    );
    assert_eq!(
        fixture.provider.0.load(Ordering::SeqCst),
        requests_before,
        "materializing the parent must not run a model turn"
    );
    let lease = fixture
        .service
        .acquire_live_session_actor_turn_boundary_lease(&session)
        .await
        .unwrap();
    let pin = fixture
        .service
        .pin_controller_client_for_actor(&lease)
        .await
        .unwrap();
    drop(lease);
    assert_eq!(pin.selection(), &selection());
    // The scripted client owns a required synthetic credential, but its
    // prepare callback deliberately publishes no native credential lifecycle.
    // Select the actual parent's actor pin, just as the ordinary class fixture
    // does, before asking native admission to accept its first input.
    meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &fixture.machine.generated_auth_lease_handle(),
        pin.selection().credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-controller-only-fixture"),
        tokio::time::Instant::now() + WAIT,
    )
    .await
    .expect("selected parent fixture credential publishes within its deadline");
    let mut event = private_event("shared-class-parent");
    event.route = fixture.source.routes().unwrap()[0].id.clone();
    event.content_sha256 = digest(b"parent input");
    let auth = event.original_authentication.clone();
    let original = fixture.source.retain_event(event).await.unwrap();
    let observation = fixture
        .source
        .submit_authenticated_original(PrivateOriginalSubmit {
            original: original.clone(),
            content: "parent input".into(),
            transport_authentication: auth,
        })
        .await
        .unwrap();
    let PrivateOriginalSubmission::Admitted {
        turn_ticket: Some(ticket),
        ..
    } = observation
    else {
        panic!(
            "actual parent admission required: {:?}",
            serde_json::to_value(&observation)
        );
    };
    let result = runtime
        .wait_for_turn(&identity, TurnTicket::parse(&ticket).unwrap(), WAIT)
        .await;
    assert!(
        matches!(result, TurnOutcome::Completed { .. }),
        "{result:?}"
    );
    assert!(
        fixture
            .source
            .binding_evidence(&original)
            .await
            .unwrap()
            .is_some()
    );
    original
}

pub(super) async fn prepare(
    fixture: &Fixture,
    parent: &PrivateOriginalRef,
    name: &str,
) -> (
    PreparedWorkflowClassSource,
    SetupWorkflowTerms,
    SetupWorkBackend,
) {
    let mut event = private_event(&format!("source-{name}"));
    event.route = fixture.route(name).id.clone();
    event.contributors = vec![parent.original.clone()];
    event.content_sha256 = digest(b"inert planned input");
    let prepared = fixture
        .gateway
        .retain_workflow_member_source(instance(name), parent, fixture.route(name), event.clone())
        .await
        .unwrap();
    let replay = fixture
        .gateway
        .retain_workflow_member_source(instance(name), parent, fixture.route(name), event.clone())
        .await
        .unwrap();
    assert!(prepared.original() == replay.original());
    assert_eq!(prepared.source_key(), replay.source_key());
    event.content_sha256 = digest(b"changed input");
    assert!(
        fixture
            .gateway
            .retain_workflow_member_source(instance(name), parent, fixture.route(name), event)
            .await
            .is_err(),
        "same source key cannot change retained content"
    );
    assert!(
        fixture
            .source
            .member_route(&prepared.route().id)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .source
            .binding_evidence(prepared.original())
            .await
            .is_err(),
        "historical source does not project an active route"
    );
    let graph = WorkGraphService::with_scope(
        Arc::new(
            SqliteWorkGraphStore::open(
                fixture
                    ._directory
                    .path()
                    .join(format!("{name}-work.sqlite")),
            )
            .unwrap(),
        ),
        fixture.source.scope.realm.clone(),
        WorkNamespace::new("shared-class").unwrap(),
    );
    let item = graph
        .create(CreateWorkItemRequest {
            title: name.into(),
            labels: ["cycle".into()].into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let item = graph
        .claim(ClaimWorkItemRequest {
            id: item.id,
            realm_id: None,
            namespace: None,
            expected_revision: item.revision,
            owner: WorkOwner {
                key: WorkOwnerKey::agent(name).unwrap(),
                display_name: None,
            },
            lease_seconds: Some(300),
            lease_expires_at: None,
        })
        .await
        .unwrap();
    let backend = SetupWorkBackend::new(
        graph,
        format!("sqlite-{name}"),
        Id::new("generation-one").unwrap(),
        &[meerkat_core::DurabilityDeclaration::durable(
            "workgraph",
            meerkat_core::DurabilityResolution::Persistent,
        )],
    )
    .unwrap();
    let route = fixture
        .source
        .project_class_source_route(&instance(name).member, prepared.route())
        .unwrap();
    let terms = SetupWorkflowTerms {
        key: Id::new(format!("workflow-{name}")).unwrap(),
        backend: backend.pin().clone(),
        parent: WorkItemRef {
            realm_id: item.realm_id,
            namespace: item.namespace,
            item_id: item.id,
        },
        parent_owner: item.owner.unwrap().key,
        cycle_label: "cycle".into(),
        requester: route.requester,
        realm: route.realm,
        authority_revision: toolkit_package::Revision::new("construction-only").unwrap(),
        planning_source: prepared.source_key().clone(),
        lead: instance(name).member.member,
        // No input is launched by this construction test and no grant is issued.
        envelope_digest: toolkit_package::ContentDigest::of_bytes(
            b"unissued construction envelope",
        ),
        deadline_ms: fixture.source.owner.clock().now().unwrap().unix_ms + 60_000,
        max_waiters: 2,
    };
    (prepared, terms, backend)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admission_fence_refuses_fresh_workflow_reservation_before_peer_preparation() {
    let mut selected = class();
    selected.purpose = CompiledMemberClassPurpose::SharedWorkflow;
    let fixture = Box::pin(Fixture::start_selected(selected, None)).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 4,
        Box::pin(async {
            let parent = parent(&fixture).await;
            let (prepared, terms, _backend) = prepare(&fixture, &parent, ADDED).await;
            assert!(
                fixture
                    .policy
                    .member_class_admission(&instance(ADDED).member.member)
                    .unwrap()
                    .is_none()
            );
            GatewayHostComposition::stop_admitting(
                fixture.gateway.as_ref(),
                GatewayHostCloseReason::Shutdown,
            )
            .unwrap();
            let result = fixture
                .gateway
                .reserve_fresh_workflow_member_class(
                    Id::new("drain-refused").unwrap(),
                    &prepared,
                    &terms,
                )
                .await;
            assert!(matches!(result, Err(HostLifetimeError::Unavailable)));
            assert!(
                fixture
                    .policy
                    .member_class_admission(&instance(ADDED).member.member)
                    .unwrap()
                    .is_none(),
                "no new Reserved handle may authorize physical peer preparation"
            );
            assert_eq!(fixture.factory.preparations.load(Ordering::SeqCst), 0);
            assert!(fixture.factory.calls.lock().unwrap().is_empty());
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match checked {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("workflow reservation drain control timed out"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_class_is_internal_requires_real_capture_and_cancellation_prevents_publication() {
    let mut selected = class();
    selected.purpose = CompiledMemberClassPurpose::SharedWorkflow;
    let captured = Arc::new(std::sync::OnceLock::new());
    let capture = captured.clone();
    let fixture = Box::pin(Fixture::start_selected_source(
        selected,
        None,
        Some(Box::new(move |inner| {
            let store = Arc::new(HeldSourceRead::new(inner));
            assert!(capture.set(store.clone()).is_ok());
            store
        })),
    ))
    .await;
    let store = captured.get().unwrap().clone();
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 5, Box::pin(async {
        fixture.gateway.bind_member_class_context(fixture.native.identity_first_context().unwrap()).unwrap();
        let parent = parent(&fixture).await;
        let (prepared, terms, _backend) = prepare(&fixture, &parent, ADDED).await;
        assert!(matches!(fixture.policy.reserve_member_class(Id::new("ordinary-refused").unwrap(), instance(ADDED)).await,
            Err(HostLifetimeError::Denied)));
        let admitted = fixture.gateway.reserve_fresh_workflow_member_class(Id::new("shared-admission").unwrap(), &prepared, &terms).await.unwrap();
        let reserved = fixture.policy.current_member_class(&admitted).unwrap();
        assert!(matches!(fixture.gateway.reserve_fresh_workflow_member_class(Id::new("shared-admission").unwrap(),
            &prepared, &terms).await, Err(HostLifetimeError::Denied)), "a replay cannot prepare a second physical resource");
        assert_eq!(fixture.policy.current_member_class(&admitted).unwrap(), reserved,
            "duplicate refusal cannot retire or modify the original reservation");
        assert!(fixture.gateway.completed_workflow_member_class(&admitted).is_err());
        let ordinary = fixture.gateway.add_member_class(&fixture.native.identity_first_context().unwrap(), admitted.clone(),
            spec(ADDED, &class().profile), fixture.route(ADDED), fixture.intent(ADDED), tokio::time::Instant::now() + WAIT,
            || async { panic!("ordinary path must never enter") }).await;
        assert!(matches!(ordinary, Err(MemberClassAddError::ReservationNotAccepted { error: HostLifetimeError::Denied })));
        let resource = CleanupProbe::new(false);
        let wrong_addressability = fixture.gateway.add_workflow_member_class(admitted.clone(), spec(ADDED, &class().profile),
            fixture.route(ADDED), fixture.intent(ADDED), tokio::time::Instant::now() + WAIT,
            || async { panic!("non-internal workflow must not enter") }, Some(resource.clone())).await;
        assert!(matches!(wrong_addressability, Err(MemberClassAddError::ReservationNotAccepted { error: HostLifetimeError::Denied })));
        assert!(resource.observations.lock().unwrap().is_empty());
        let mut internal = spec(ADDED, &class().profile);
        internal.addressability = AgentAddressability::InternalOnly;
        let policy = fixture.policy.clone();
        let exact = admitted.clone();
        let result = fixture.gateway.add_workflow_member_class(admitted.clone(), internal,
            fixture.route(ADDED), fixture.intent(ADDED), tokio::time::Instant::now() + WAIT,
            move || async move {
                let actual = policy.current_member_class(&exact)?;
                if actual.route.is_none() || actual.native_surface.is_none()
                    || !matches!(actual.phase, crate::installed_host::dynamic_members::MemberClassAdmissionPhase::Prepared { .. })
                { return Err(HostLifetimeError::Denied); }
                Ok(())
            }, Some(resource.clone())).await.unwrap();
        let outcome = match result { MemberClassAddStart::Settled(outcome) => outcome,
            MemberClassAddStart::Unsettled(settlement) => settlement.settled().await.unwrap() };
        assert!(outcome.authorization.is_ok() && outcome.cleanup.is_ok(), "{outcome:?}");
        assert!(matches!(outcome.native.as_ref().unwrap().outcomes.get(&AgentIdentity::parse(ADDED).unwrap()),
            Some(RestoreOutcome::Created { .. })), "exact Created required: {outcome:?}");
        assert_eq!(*resource.observations.lock().unwrap(), ["created"]);
        assert_eq!(resource.released.load(Ordering::SeqCst), 0, "Created resource is retained");
        let receipt = fixture.gateway.completed_workflow_member_class(&admitted).unwrap();
        assert_eq!(receipt.member.member, instance(ADDED).member.member);
        assert!(!receipt.native_surface.is_empty());
        // Cold selection reads the exact current canonical decision. It cannot
        // issue another Add or use a changed saved record as authorization.
        let checkpoint = fixture.policy.member_class_checkpoint(&receipt.member.member).await.unwrap();
        let restored = fixture.policy.restore_workflow_member_class(&receipt.admission).unwrap();
        assert_eq!(fixture.policy.current_member_class(&restored).unwrap(), receipt.admission);
        assert!(restored.claim_add().is_err());
        assert!(restored.dispatcher().is_none(), "actual construction supplies callback custody");
        for field in ["id", "workflow", "session", "phase"] {
            let mut changed = receipt.admission.clone();
            match field {
                "id" => changed.id = Id::new("foreign-admission").unwrap(),
                "workflow" => changed.workflow = None,
                "session" => changed.destination.as_mut().unwrap().context = EvidenceId::new("foreign-session").unwrap(),
                "phase" => changed.phase = crate::installed_host::dynamic_members::MemberClassAdmissionPhase::Reserved,
                _ => unreachable!(),
            }
            assert!(fixture.policy.restore_workflow_member_class(&changed).is_err());
        }
        fixture._current.store(false, Ordering::SeqCst);
        assert!(fixture.policy.restore_workflow_member_class(&receipt.admission).is_err(),
            "saved Active does not replace current application authorization");
        fixture._current.store(true, Ordering::SeqCst);
        assert_eq!(fixture.policy.member_class_checkpoint(&receipt.member.member).await.unwrap(), checkpoint,
            "selection neither changes the canonical decision nor issues another reservation");
        let session = receipt.workflow_member.session().unwrap();
        assert!(fixture.source.current_console_routes().unwrap().iter().all(|route| route.id != prepared.route().id));
        let before_source = fixture.source.store.get(prepared.source_key()).await.unwrap().unwrap();
        let current = fixture.policy.restore_workflow_member_class(&receipt.admission).unwrap();
        assert!(fixture.source.restore_workflow_class_source(Arc::new(current), &AgentIdentity::parse(ADDED).unwrap()).await.is_err(),
            "an Active class without an actually bound historical G input cannot acquire restored source custody");
        assert_eq!(fixture.source.store.get(prepared.source_key()).await.unwrap().unwrap(), before_source);
        // Parser/origin comparison only: these descriptors are never used to
        // construct a helper, native catalog, plan or restored source owner.
        let mut record = fixture.source.decode_record(&before_source, prepared.original()).unwrap();
        record.planning_source_evidence = Some(crate::private_originals::planning_source_evidence::PlanningSourceEvidence::Lead {
            parent: parent.clone(),
        });
        let mut without_helper = before_source.clone();
        without_helper.data.body = serde_json::to_value(&record).unwrap();
        without_helper.content_digest = without_helper.data.digest().unwrap();
        record.late_workflow_helper = Some(crate::private_originals::planning_source_evidence::LateWorkflowHelperProvenance {
            admission: receipt.admission.clone(),
            server: meerkat_core::McpServerConfig::stdio("origin-comparison-only", "never-executed", vec![], Default::default()),
        });
        let mut with_helper = before_source.clone();
        with_helper.data.body = serde_json::to_value(&record).unwrap();
        with_helper.content_digest = with_helper.data.digest().unwrap();
        assert!(super::super::decode_source_record(&with_helper).is_ok());
        assert!(!super::super::same_source_origin(&without_helper, &with_helper));
        assert!(!super::super::same_source_origin(&with_helper, &without_helper));
        assert!(super::super::same_source_origin(&with_helper, &with_helper));
        for change in 0..2 {
            let mut changed = record.clone();
            let provenance = changed.late_workflow_helper.as_mut().unwrap();
            if change == 0 { provenance.admission.id = Id::new("different-historical-admission").unwrap(); }
            else {
                let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) = &mut provenance.server.transport else {
                    panic!("origin comparison fixture requires its stdio descriptor");
                };
                stdio.args.push("different-historical-server".into());
            }
            let mut changed_row = with_helper.clone();
            changed_row.data.body = serde_json::to_value(changed).unwrap();
            changed_row.content_digest = changed_row.data.digest().unwrap();
            assert!(super::super::decode_source_record(&changed_row).is_ok());
            assert!(!super::super::same_source_origin(&with_helper, &changed_row));
        }
        let before_history = fixture.service.read_history(&session,
            SessionHistoryQuery { offset: 0, limit: Some(100) }).await.unwrap();
        assert!(!before_history.has_more);
        let before_history = serde_json::to_value(before_history.messages).unwrap();
        assert_eq!(fixture.machine.meerkat_machine_spine_snapshot(&session).await.unwrap().ledger.input_count, 0);
        let before_requests = fixture.provider.0.load(Ordering::SeqCst);
        let before_calls = fixture.factory.calls.lock().unwrap().clone();
        // The ordinary submission projects native errors conservatively. Its
        // unavailable outcome alone cannot establish nonadmission; assert the
        // actual external door and unchanged native/source state separately.
        let event = fixture.source.event(prepared.original()).await.unwrap();
        let observation = fixture.source.submit_authenticated_original(PrivateOriginalSubmit { original: prepared.original().clone(),
            content: "inert planned input".into(), transport_authentication: event.original_authentication }).await.unwrap();
        assert!(matches!(observation, PrivateOriginalSubmission::NativeOutcomeUnavailable),
            "unexpected ordinary submission: {:?}", serde_json::to_value(&observation));
        let runtime = fixture.source.bound_identity_runtime().unwrap();
        let external = runtime.send_tracked(&AgentIdentity::parse(ADDED).unwrap(),
            &meerkat_core::ContentInput::Text("foreign external input".into())).await;
        assert!(matches!(external, Err(IdentityRuntimeError::NotAddressable(_))), "{external:?}");
        assert_eq!(fixture.machine.meerkat_machine_spine_snapshot(&session).await.unwrap().ledger.input_count, 0);
        let after_history = fixture.service.read_history(&session,
            SessionHistoryQuery { offset: 0, limit: Some(100) }).await.unwrap();
        assert!(!after_history.has_more);
        assert_eq!(serde_json::to_value(after_history.messages).unwrap(), before_history);
        assert_eq!(fixture.provider.0.load(Ordering::SeqCst), before_requests);
        assert_eq!(*fixture.factory.calls.lock().unwrap(), before_calls);
        assert_eq!(fixture.source.store.get(prepared.source_key()).await.unwrap(), Some(before_source));
        assert!(fixture.source.binding_evidence(prepared.original()).await.unwrap().is_none());
        // Hold the actual source store read after it has returned its durable
        // row, retire this same native-created admission, then release the row.
        // The final source fence must observe retirement, not its old snapshot.
        *store.held_key.lock().unwrap() = Some(prepared.source_key().clone());
        let route = fixture.route(ADDED);
        let (checked, ()) = tokio::join!(
            fixture.source.check_workflow_class_source(&admitted, &route),
            async {
                store.entered.notified().await;
                admitted.retire().await.unwrap();
                store.release.add_permits(1);
            },
        );
        assert!(matches!(checked, Err(HostLifetimeError::Denied)), "retirement won during source read: {checked:?}");
        assert!(fixture.gateway.completed_workflow_member_class(&admitted).is_err());
        assert!(fixture.policy.restore_workflow_member_class(&receipt.admission).is_err(),
            "retirement also invalidates a previously selected cold record");

        let (aborted_source, aborted_terms, _aborted_backend) = prepare(&fixture, &parent, "project-aborted").await;
        let aborted = fixture.gateway.reserve_workflow_member_class(Id::new("shared-aborted").unwrap(), &aborted_source, &aborted_terms).await.unwrap();
        let source = fixture.source.clone();
        let original = parent.clone();
        let mut internal = spec("project-aborted", &class().profile);
        internal.addressability = AgentAddressability::InternalOnly;
        let result = fixture.gateway.add_workflow_member_class(aborted.clone(), internal,
            fixture.route("project-aborted"), fixture.intent("project-aborted"), tokio::time::Instant::now() + WAIT,
            move || async move { source.record_operation_cancelled(&original).await.map_err(|_| HostLifetimeError::Unavailable) }, None).await.unwrap();
        let outcome = match result { MemberClassAddStart::Settled(outcome) => outcome,
            MemberClassAddStart::Unsettled(settlement) => settlement.settled().await.unwrap() };
        assert!(outcome.authorization.is_err() && outcome.cleanup.is_ok(), "{outcome:?}");
        assert!(fixture.gateway.completed_workflow_member_class(&aborted).is_err());
        assert!(fixture.gateway.reserve_workflow_member_class(Id::new("cancelled-retry").unwrap(), &aborted_source, &aborted_terms).await.is_err());
        assert!(fixture.factory.calls.lock().unwrap().is_empty(), "construction never grants callback execution");
    }))).catch_unwind().await;
    store.release.add_permits(1);
    fixture.close().await;
    checked
        .expect("workflow fixture panicked")
        .expect("workflow fixture timed out");
}

/// Test-only observation pause over the real source SQLite owner. No source or
/// policy record is invented or changed by this decorator.
pub(super) struct HeldSourceRead {
    inner: Arc<dyn toolkit_store::ToolkitStore>,
    pub(super) held_key: Mutex<Option<StoreKey>>,
    pub(super) entered: tokio::sync::Notify,
    pub(super) release: tokio::sync::Semaphore,
}
impl HeldSourceRead {
    pub(super) fn new(inner: Arc<dyn toolkit_store::ToolkitStore>) -> Self {
        Self {
            inner,
            held_key: Mutex::new(None),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        }
    }
}
#[async_trait::async_trait]
impl toolkit_store::ToolkitStore for HeldSourceRead {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        self.inner.now_ms().await
    }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        self.inner.admit(request).await
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        let actual = self.inner.get(key).await;
        let pause = {
            let mut selected = self.held_key.lock().unwrap();
            if selected.as_ref() == Some(key) {
                selected.take();
                true
            } else {
                false
            }
        };
        if pause {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        actual
    }
    async fn list_records(
        &self,
        scope: &StoreScope,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredRecord>> {
        self.inner.list_records(scope, after, limit).await
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        self.inner.compare_and_swap(transition).await
    }
    async fn claim_record(
        &self,
        key: &StoreKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<RecordLease> {
        self.inner.claim_record(key, revision, worker, ttl).await
    }
    async fn renew_record_claim(&self, lease: &RecordLease, ttl: u64) -> StoreResult<RecordLease> {
        self.inner.renew_record_claim(lease, ttl).await
    }
    async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease> {
        self.inner.recover_record_claim(prior).await
    }
    async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()> {
        self.inner.release_record_claim(lease).await
    }
    async fn read_events(
        &self,
        key: &StoreKey,
        after: u64,
        limit: u32,
    ) -> StoreResult<Vec<StoredEvent>> {
        self.inner.read_events(key, after, limit).await
    }
    async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>> {
        self.inner.get_effect(key).await
    }
    async fn list_effects(
        &self,
        key: &StoreKey,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredEffect>> {
        self.inner.list_effects(key, after, limit).await
    }
    async fn claim_effect(
        &self,
        key: &EffectKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<EffectClaimOutcome> {
        self.inner.claim_effect(key, revision, worker, ttl).await
    }
    async fn acknowledge_effect(
        &self,
        lease: &EffectLease,
        outcome: EffectOutcome,
    ) -> StoreResult<StoredEffect> {
        self.inner.acknowledge_effect(lease, outcome).await
    }
    async fn claim_reconciliation(
        &self,
        key: &EffectKey,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<StoredEffect> {
        self.inner.claim_reconciliation(key, worker, ttl).await
    }
    async fn reconcile_effect(
        &self,
        lease: &EffectLease,
        resolution: Reconciliation,
    ) -> StoreResult<StoredEffect> {
        self.inner.reconcile_effect(lease, resolution).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_workflow_preflight_joins_resource_cleanup_without_native_entry() {
    let mut selected = class();
    selected.purpose = CompiledMemberClassPurpose::SharedWorkflow;
    let fixture = Box::pin(Fixture::start_selected(selected, None)).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 4,
        Box::pin(async {
            fixture
                .gateway
                .bind_member_class_context(fixture.native.identity_first_context().unwrap())
                .unwrap();
            let parent = parent(&fixture).await;
            let (prepared, terms, _backend) = prepare(&fixture, &parent, "project-refused").await;
            let admitted = fixture
                .gateway
                .reserve_workflow_member_class(
                    Id::new("shared-preflight").unwrap(),
                    &prepared,
                    &terms,
                )
                .await
                .unwrap();
            let mut route = fixture.route("project-refused");
            route.id = fixture
                .lifetime
                .selected_policy()
                .native_host
                .as_ref()
                .unwrap()
                .routes[0]
                .id
                .clone();
            let mut internal = spec("project-refused", &class().profile);
            internal.addressability = AgentAddressability::InternalOnly;
            let cleanup = CleanupProbe::new(false);
            let result = fixture
                .gateway
                .add_workflow_member_class(
                    admitted.clone(),
                    internal,
                    route,
                    fixture.intent("project-refused"),
                    tokio::time::Instant::now() + WAIT,
                    || async { panic!("preflight refusal never enters finalizer") },
                    Some(cleanup.clone()),
                )
                .await;
            assert!(
                matches!(
                    result,
                    Err(MemberClassAddError::Preflight {
                        error: HostLifetimeError::Denied,
                        cleanup: Ok(())
                    })
                ),
                "{result:?}"
            );
            assert_eq!(*cleanup.observations.lock().unwrap(), ["preflight"]);
            assert_eq!(cleanup.released.load(Ordering::SeqCst), 1);
            assert_eq!(
                fixture
                    .policy
                    .observe_member_class(&instance("project-refused").member.member)
                    .await
                    .unwrap()
                    .admission
                    .phase,
                crate::installed_host::dynamic_members::MemberClassAdmissionPhase::Retired
            );
            assert_eq!(fixture.native.mob_handle().list_members().await.len(), 1);
            assert_eq!(fixture.factory.preparations.load(Ordering::SeqCst), 0);
            assert!(fixture.factory.calls.lock().unwrap().is_empty());
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    checked
        .expect("workflow resource cleanup fixture panicked")
        .expect("workflow resource cleanup fixture timed out");
}

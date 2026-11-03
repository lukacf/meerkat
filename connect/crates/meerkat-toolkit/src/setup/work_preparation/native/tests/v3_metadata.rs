//! Actual preparation/publication owner plus actual Toolkit/WorkGraph SQLite.
//! These are not host adoption, helper launch, C1 or flow-admission fixtures.
use super::*;
use meerkat_core::service::WorkGraphNamespaceGrant;
use meerkat_core::storage_durability::{
    DurabilityClass, DurabilityDeclaration, DurabilityResolution,
};
use meerkat_workgraph::{
    ClaimWorkItemRequest, MemoryWorkGraphStore, SqliteWorkGraphStore, WorkGraphEventFilter,
    WorkGraphEventKind, WorkGraphMachine, WorkGraphService, WorkGraphStore, WorkOwner,
    WorkOwnerKey, WorkStatus,
};

fn real_target(f: &Fixture) -> (Arc<SqliteWorkGraphStore>, WorkGraphService) {
    let store =
        Arc::new(SqliteWorkGraphStore::open(f.directory.path().join("workgraph.sqlite")).unwrap());
    let service = WorkGraphService::with_namespace_grant(
        store.clone(),
        WorkGraphNamespaceGrant::new("work-realm", "setup").unwrap(),
    )
    .unwrap();
    (store, service)
}

async fn publish(f: &Fixture, proposal: &NativePreparedWorkProposal) -> PublishedSetupWork {
    f.bridge(&f.store)
        .publish_prepared_work(
            &f.snapshot,
            proposal,
            f.child_evidence(proposal, f.now),
            f.access(&f.context(f.now)),
            f.conditions(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn sqlite_v3_recovery_pins_backend_and_preserves_original_separately_from_inert_pair() {
    let f = Fixture::new().await;
    let proposal = f.propose("PRIVATE ORIGINAL REQUEST").await;
    let published = publish(&f, &proposal).await;
    let original = canonical_json(published.work_request()).unwrap();
    let pair = (
        published.prepared_item().clone(),
        published.prepared_created_event().clone(),
    );
    assert_eq!(published.work_request().title, "PRIVATE ORIGINAL REQUEST");
    assert_eq!(
        published.work_request().description.as_deref(),
        Some("Exact bytes.\n  Keep these spaces.")
    );
    assert_eq!(pair.0.title, "Toolkit setup preparation");
    assert_eq!(pair.0.status, WorkStatus::Blocked);
    assert!(pair.0.description.is_none());
    assert!(pair.0.labels.is_empty());
    assert!(pair.0.evidence_refs.is_empty());
    assert!(pair.0.claim.is_none());
    assert!(pair.0.owner.is_none());
    assert_eq!(pair.0.external_refs.len(), 1);
    assert_eq!(
        pair.0.external_refs[0].kind,
        "meerkat_toolkit.setup_work_correlation.v3"
    );
    assert!(pair.0.external_refs[0].url.is_none());
    ContentDigest::parse(pair.0.external_refs[0].id.clone()).unwrap();
    let encoded = String::from_utf8(canonical_json(&pair).unwrap()).unwrap();
    for secret in [
        "PRIVATE ORIGINAL REQUEST",
        "Exact bytes",
        "input-owner",
        "reply-one",
        "private-one",
        "person",
    ] {
        assert!(!encoded.contains(secret), "metadata leaked {secret}");
    }
    assert_eq!(pair.1.seq, None);
    assert!(pair.1.facts.is_empty());

    let toolkit = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
    let selected = selected_backend(f.directory.path(), "backend-one");
    let key = published.key().clone();
    let before = toolkit.get(&key).await.unwrap().unwrap();
    let recovered = f
        .bridge(&toolkit)
        .load_prepared_work_exact(
            &operation_id(f.snapshot.request()).unwrap(),
            &published.child_reference().clone(),
            f.access(&f.context(f.now)),
            &selected,
        )
        .await
        .unwrap();
    assert_eq!(canonical_json(recovered.work_request()).unwrap(), original);
    assert_eq!(recovered.prepared_item(), &pair.0);
    assert_eq!(recovered.prepared_created_event(), &pair.1);
    assert_eq!(recovered.backend_pin(), selected.pin());
    assert_eq!(recovered.digest(), published.digest());

    let replacement = selected_backend(f.directory.path(), "backend-two");
    assert!(matches!(
        f.bridge(&toolkit)
            .load_prepared_work_exact(
                &operation_id(f.snapshot.request()).unwrap(),
                &published.child_reference().clone(),
                f.access(&f.context(f.now)),
                &replacement,
            )
            .await,
        Err(SetupError::Store(StoreError::AdmissionConflict))
    ));
    let changed_selection = f
        .bridge(&toolkit)
        .propose_native_work(
            &f.snapshot,
            f.access(&f.context(f.now)),
            &Preparer {
                title: "PRIVATE ORIGINAL REQUEST".into(),
                expires: f.deadline,
            },
            &replacement,
        )
        .await
        .unwrap();
    assert!(matches!(
        f.bridge(&toolkit)
            .publish_prepared_work(
                &f.snapshot,
                &changed_selection,
                f.child_evidence(&changed_selection, f.now),
                f.access(&f.context(f.now)),
                f.conditions(),
            )
            .await,
        Err(SetupError::Store(StoreError::AdmissionConflict))
    ));
    assert_eq!(toolkit.get(&key).await.unwrap().unwrap(), before);
    let (target, _) = real_target(&f);
    assert!(
        target
            .list_events(WorkGraphEventFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        target
            .get_item(
                "work-realm",
                &WorkNamespace::new("setup").unwrap(),
                &pair.0.id
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_native_candidates_select_one_sqlite_pair_without_target_insertion() {
    let f = Arc::new(Fixture::new().await);
    let first = f.propose("same exact original").await;
    let second = f.propose("same exact original").await;
    let first_id = first.metadata.item().id.clone();
    let second_id = second.metadata.item().id.clone();
    assert_ne!(
        first_id, second_id,
        "the fixture must exercise different random candidates"
    );
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut handles = Vec::new();
    for candidate in [first, second] {
        let f = f.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            let connection = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
            let child = f.child_evidence(&candidate, f.now);
            barrier.wait().await;
            f.bridge(&connection)
                .publish_prepared_work(
                    &f.snapshot,
                    &candidate,
                    child,
                    f.access(&f.context(f.now)),
                    f.conditions(),
                )
                .await
        }));
    }
    // Join both actual publication tasks before assertions can unwind.
    let left = handles.remove(0).await;
    let right = handles.remove(0).await;
    let left = left.unwrap().unwrap();
    let right = right.unwrap().unwrap();
    assert_eq!(left.prepared_item(), right.prepared_item());
    assert_eq!(
        left.prepared_created_event(),
        right.prepared_created_event()
    );
    assert_eq!(left.digest(), right.digest());
    assert!([first_id, second_id].contains(&left.prepared_item().id));
    assert_eq!(f.store.get(left.key()).await.unwrap().unwrap().revision, 1);
    let (target, _) = real_target(&f);
    assert!(
        target
            .list_events(WorkGraphEventFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn self_consistent_but_wrong_native_pair_is_rejected_after_sqlite_reopen() {
    for fault in ["realm", "title", "open", "correlation"] {
        let f = Fixture::new().await;
        let proposal = f.propose("immutable original").await;
        let child = f.child_evidence(&proposal, f.now);
        let reference = child.original_work().clone();
        let mut conditions = f.conditions().owners;
        conditions.push(RecordCondition {
            key: f.snapshot.key().clone(),
            revision: f.snapshot.stored.revision,
            content_digest: f.snapshot.stored.content_digest.clone(),
            claim: None,
            dispatch_claim: Some(f.lease.clone()),
        });
        let mut record = NativePreparedRecord {
            helper_source: None,
            work: proposal.work.clone(),
            metadata: proposal.metadata.clone(),
            delegation: proposal.delegation.clone(),
            child,
            basis: publication_basis(&conditions, proposal.key()).unwrap(),
        };
        // Make a genuinely native, self-consistent but wrong pair. The valid
        // outer digest and event.item must not suffice for recovery.
        let mut request: CreateWorkItemRequest =
            serde_json::from_value(serde_json::to_value(proposal.metadata.item()).unwrap())
                .unwrap();
        match fault {
            "realm" => request.realm_id = Some("another-realm".into()),
            "title" => request.title = "not the fixed metadata title".into(),
            "open" => request.status = Some(WorkStatus::Open),
            "correlation" => {
                request.external_refs[0].id =
                    ContentDigest::of_bytes(b"other operation").to_string()
            }
            _ => unreachable!(),
        }
        let arena = Arc::new(MemoryWorkGraphStore::new());
        let service = WorkGraphService::with_namespace_grant(
            arena.clone(),
            WorkGraphNamespaceGrant::new(
                request.realm_id.clone().unwrap(),
                request.namespace.as_ref().unwrap().as_str(),
            )
            .unwrap(),
        )
        .unwrap();
        let item = service.create(request).await.unwrap();
        WorkGraphMachine::validate_item_projection(&item).unwrap();
        let mut events = arena
            .list_events(WorkGraphEventFilter::default())
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        let mut event = events.remove(0);
        event.seq = None;
        event.facts.clear();
        assert_eq!(event.payload["item"], serde_json::to_value(&item).unwrap());
        let mut metadata = serde_json::to_value(&record.metadata).unwrap();
        metadata["item"] = serde_json::to_value(item).unwrap();
        metadata["event"] = serde_json::to_value(event).unwrap();
        record.metadata = serde_json::from_value(metadata).unwrap();
        let saved = f
            .store
            .admit(AdmissionRequest {
                key: proposal.key.clone(),
                admission_key: proposal.admission_key.clone(),
                data: RecordData {
                    schema: SCHEMA.into(),
                    version: VERSION,
                    body: serde_json::to_value(record).unwrap(),
                },
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap()
            .record()
            .clone();
        assert_eq!(saved.content_digest, saved.data.digest().unwrap());
        let toolkit = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
        assert!(
            matches!(
                f.bridge(&toolkit)
                    .load_prepared_work_exact(
                        &operation_id(f.snapshot.request()).unwrap(),
                        &reference,
                        f.access(&f.context(f.now)),
                        &f.backend,
                    )
                    .await,
                Err(SetupError::InvalidRecord)
            ),
            "{fault}"
        );
        assert!(
            matches!(
                f.bridge(&toolkit)
                    .load_prepared_child_exact(
                        &operation_id(f.snapshot.request()).unwrap(),
                        &reference,
                        &f.authority
                    )
                    .await,
                Err(SetupError::InvalidRecord)
            ),
            "child integrity must also reject {fault}"
        );
        assert_eq!(toolkit.get(proposal.key()).await.unwrap().unwrap(), saved);
        let (target, _) = real_target(&f);
        assert!(
            target
                .list_events(WorkGraphEventFilter::default())
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn exact_blocked_pair_has_no_ready_fact_claim_or_binding_in_actual_sqlite() {
    let f = Fixture::new().await;
    let published = publish(&f, &f.propose("private task").await).await;
    let (target, host) = real_target(&f);
    // Test-only insertion of the exact saved pair. No production materializer
    // or launch path is enabled by this preparation change.
    let inserted = target
        .insert_item(
            published.prepared_item().clone(),
            published.prepared_created_event().clone(),
        )
        .await
        .unwrap();
    assert_eq!(&inserted, published.prepared_item());
    assert!(host.ready(Default::default()).await.unwrap().is_empty());
    let before = target
        .list_events(WorkGraphEventFilter::default())
        .await
        .unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].kind, WorkGraphEventKind::Created);
    assert!(before[0].seq.is_some());
    assert!(before[0].facts.is_empty());
    assert!(
        host.list_attention(Default::default())
            .await
            .unwrap()
            .attention
            .is_empty()
    );
    assert!(
        target
            .list_execution_bindings(Default::default())
            .await
            .unwrap()
            .is_empty()
    );
    let claim = ClaimWorkItemRequest {
        id: inserted.id.clone(),
        realm_id: None,
        namespace: None,
        expected_revision: inserted.revision,
        owner: WorkOwner {
            key: WorkOwnerKey::agent("ordinary-worker").unwrap(),
            display_name: None,
        },
        lease_seconds: Some(30),
        lease_expires_at: None,
    };
    assert!(
        host.claim(claim.clone()).await.is_err(),
        "Blocked is not claimable"
    );
    // A worker's actual namespace grant cannot be widened by naming setup.
    // Do not hand workers the host setup service or its execution bridge.
    let worker = WorkGraphService::with_namespace_grant(
        target.clone(),
        WorkGraphNamespaceGrant::new("work-realm", "ordinary-worker").unwrap(),
    )
    .unwrap();
    let mut wrong_scope = claim;
    wrong_scope.namespace = Some(WorkNamespace::new("setup").unwrap());
    assert!(worker.claim(wrong_scope).await.is_err());
    let mutation = serde_json::from_value(serde_json::json!({
        "id": inserted.id, "namespace": "setup", "expected_revision": inserted.revision,
        "title": "worker must not mutate this host preparation"
    }))
    .unwrap();
    assert!(worker.update(mutation).await.is_err());
    assert!(worker.get(None, None, inserted.id.clone()).await.is_err());
    assert_eq!(
        target
            .list_events(WorkGraphEventFilter::default())
            .await
            .unwrap(),
        before
    );
    let reopened = SqliteWorkGraphStore::open(f.directory.path().join("workgraph.sqlite")).unwrap();
    assert_eq!(
        reopened
            .get_item(
                "work-realm",
                &WorkNamespace::new("setup").unwrap(),
                &inserted.id
            )
            .await
            .unwrap(),
        Some(inserted)
    );
    // This proves inert creation and one narrow native scope refusal. A trusted
    // host can still launch nonterminal work through separate flow admission;
    // Blocked is deliberately not claimed to be an authorization boundary.
}

#[tokio::test]
async fn selected_backend_requires_one_exact_durable_declaration_and_matching_scope() {
    let f = Fixture::new().await;
    let (_, service) = real_target(&f);
    let good = DurabilityDeclaration::durable("workgraph", DurabilityResolution::Persistent);
    let absent = vec![];
    let duplicate = vec![good.clone(), good.clone()];
    let ephemeral = vec![DurabilityDeclaration::durable(
        "workgraph",
        DurabilityResolution::DeclaredEphemeral,
    )];
    let missing = vec![DurabilityDeclaration::durable(
        "workgraph",
        DurabilityResolution::NonPersistent,
    )];
    let cache = vec![DurabilityDeclaration {
        class: DurabilityClass::RebuildableCache,
        ..good.clone()
    }];
    for declarations in [absent, duplicate, ephemeral, missing, cache] {
        assert!(
            SetupWorkBackend::new(
                service.clone(),
                "fixture-provider".into(),
                id("generation"),
                &declarations
            )
            .is_err()
        );
    }
    let selected = SetupWorkBackend::new(
        service,
        "fixture-provider".into(),
        id("generation"),
        &[good],
    )
    .unwrap();
    assert_eq!(selected.pin().realm().as_str(), "work-realm");
    assert_eq!(selected.pin().namespace().as_str(), "setup");
    let wrong =
        HostWorkGraphScope::new(id("wrong-realm"), WorkNamespace::new("setup").unwrap()).unwrap();
    assert!(matches!(
        selected.check_scope(&wrong),
        Err(SetupError::WrongScope)
    ));
    let wrong =
        HostWorkGraphScope::new(id("work-realm"), WorkNamespace::new("workers").unwrap()).unwrap();
    assert!(matches!(
        selected.check_scope(&wrong),
        Err(SetupError::WrongScope)
    ));
}

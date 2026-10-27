//! SQLite effect/publication controls with explicit historical source fixtures.
//! These exercise real WorkGraph keyed storage, not native ingress authority,
//! helper grant issuance, controller execution or a qualified product flow.
use super::*;
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_workgraph::WorkAdmissionOutcome;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct UnboundPreparer<'a> {
    title: &'a str,
    expires: u64,
}
impl SetupWorkPreparer for UnboundPreparer<'_> {
    fn prepare(
        &self,
        request: &ValidatedSetupRequest,
    ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
        let mut result = Preparer {
            title: self.title.into(),
            expires: self.expires,
        }
        .prepare(request)?;
        result.delegation = None;
        Ok(result)
    }
}
async fn prepare(f: &Fixture, title: &str) -> NativeWorkPreparation {
    f.bridge(&f.store)
        .prepare_native_work(
            &f.snapshot,
            f.access(&f.context(f.now)),
            &UnboundPreparer {
                title,
                expires: f.deadline,
            },
            &f.backend,
        )
        .await
        .unwrap()
}
fn admitted_request(preparation: &NativeWorkPreparation) -> CreateWorkItemRequest {
    let mut request = preparation.work_request().clone();
    request.realm_id = Some(preparation.backend_pin().realm().to_string());
    request.namespace = Some(preparation.backend_pin().namespace().clone());
    request
}
async fn graph_admit(
    f: &Fixture,
    preparation: &NativeWorkPreparation,
) -> NativeWorkObservation<()> {
    let request = admitted_request(preparation);
    let service = f.backend.service();
    let admission = service
        .admission_identity(preparation.admission_key().clone(), request.clone())
        .unwrap();
    let item = service
        .create_idempotent(preparation.admission_key().clone(), request.clone())
        .await
        .unwrap();
    assert!(!matches!(item, WorkAdmissionOutcome::Conflict { .. }));
    NativeWorkObservation {
        realm: preparation.backend_pin().realm().to_string(),
        namespace: preparation.backend_pin().namespace().clone(),
        item: item.item().unwrap().id.clone(),
        admission,
        admitted_request: request,
        retained: (),
    }
}
async fn helper_fixture(
    f: &Fixture,
    preparation: &NativeWorkPreparation,
    observation: &NativeWorkObservation<()>,
) -> (
    NativePreparedWorkProposal,
    SetupRequestEvidence,
    NativeHelperSource,
) {
    helper_fixture_with_authority(
        f, preparation, observation,
        f.authority.association().candidate().ingress_namespace.ingress.authority.clone(),
    ).await
}
async fn helper_fixture_with_authority(
    f: &Fixture,
    preparation: &NativeWorkPreparation,
    observation: &NativeWorkObservation<()>,
    authority: meerkat_core::PrincipalRef,
) -> (
    NativePreparedWorkProposal,
    SetupRequestEvidence,
    NativeHelperSource,
) {
    let identity = format!(
        "setup-helper/{}:{}/{}",
        observation.namespace.as_str().len(),
        observation.namespace,
        observation.item
    );
    let mut delegation = Preparer {
        title: "unused".into(),
        expires: f.deadline,
    }
    .prepare(f.snapshot.request())
    .unwrap()
    .delegation
    .unwrap();
    delegation.original_work.authority = authority;
    delegation.original_work.work = EvidenceId::new(&identity).unwrap();
    let proposal = f
        .bridge(&f.store)
        .bind_native_work(
            &f.snapshot,
            preparation,
            delegation,
            f.access(&f.context(f.now)),
        )
        .await
        .unwrap();
    let child = f.child_evidence(&proposal, f.now);
    let mut c = child.association().candidate().clone();
    // The helper has its own actual source identity. Copying the parent's auth
    // would deliberately fail the new helper publication contract.
    c.ingress_actor = principal("helper-ingress", PrincipalKind::ServiceAccount);
    c.original_authentication = native_fixture::event(&identity);
    c.original_authentication.resource.domain = c.ingress_namespace.ingress.clone();
    c.root_event = c.original_authentication.clone();
    let child = SetupRequestEvidence::new(
        InputAuthorityAssociation::new(c).unwrap(),
        child.constraints().clone(),
    )
    .unwrap();
    let source = f
        .store
        .admit(AdmissionRequest {
            key: StoreKey {
                realm: "tenant".into(),
                owner: "toolkit/helper-originals/deployment".into(),
                id: identity.clone(),
            },
            admission_key: identity,
            data: RecordData {
                schema: "test.explicit_helper_source_history".into(),
                version: 1,
                body: serde_json::to_value(child.association()).unwrap(),
            },
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap()
        .record()
        .clone();
    let fact = NativeHelperSource {
        source: RecordCondition {
            key: source.key,
            revision: source.revision,
            content_digest: source.content_digest,
            claim: None,
            dispatch_claim: None,
        },
        realm: observation.realm.clone(),
        namespace: observation.namespace.clone(),
        item: observation.item.clone(),
        admission: observation.admission.clone(),
        effect: preparation.effect_key().clone(),
        preparation_request: preparation.work_request().clone(),
        admitted_request: observation.admitted_request.clone(),
        association: child.association().clone(),
        contributors: vec![f.authority.association().clone()],
    };
    (proposal, child, fact)
}
fn conditions(
    f: &Fixture,
    fact: &NativeHelperSource,
    lease: &EffectLease,
) -> PreparedWorkPublicationConditions {
    let mut result = f.conditions();
    result.dispatch_claim = lease.clone();
    result.owners.push(fact.source.clone());
    result
}

#[tokio::test]
async fn pre_worker_input_freezes_request_before_real_helper_identity_and_reopens() {
    let f = Fixture::with_ingress_authority("helper-source").await;
    let preparation = prepare(&f, "first request").await;
    let restarted = prepare(&f, "changed configuration is ignored").await;
    assert_eq!(
        canonical_json(preparation.work_request()).unwrap(),
        canonical_json(restarted.work_request()).unwrap()
    );
    assert_eq!(preparation.input_record, restarted.input_record);
    assert_eq!(preparation.effect_key(), &f.lease.key);
    assert_eq!(
        preparation.admission_key().as_str(),
        f.bridge(&f.store)
            .original_work_effect(&f.snapshot)
            .await
            .unwrap()
            .intent
            .idempotency_key
    );
    assert!(f.store.get(preparation.key()).await.unwrap().is_none());
    let observed = graph_admit(&f, &preparation).await;
    let (proposal, child, fact) = helper_fixture(&f, &preparation, &observed).await;
    let parent = f.authority.association().candidate();
    let helper = child.association().candidate();
    assert!(helper.original_work.authority != parent.original_work.authority);
    assert!(helper.original_work.authority == parent.ingress_namespace.ingress.authority);
    assert!(
        child.association().candidate().original_authentication
            != f.authority
                .association()
                .candidate()
                .original_authentication
    );
    let published = f
        .bridge(&f.store)
        .publish_prepared_helper_work(
            &f.snapshot,
            &proposal,
            child.clone(),
            fact.clone(),
            f.access(&f.context(f.now)),
            conditions(&f, &fact, &f.lease),
        )
        .await
        .unwrap();
    assert_eq!(published.child(), &child);
    let reopened = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
    let actual = f
        .bridge(&reopened)
        .load_prepared_work_exact(
            &operation_id(f.snapshot.request()).unwrap(),
            child.original_work(),
            f.access(&f.context(f.now)),
            &f.backend,
        )
        .await
        .unwrap();
    assert_eq!(published.digest(), actual.digest());
}

#[tokio::test]
async fn helper_publication_refuses_parent_or_foreign_source_authority() {
    for authority in ["source", "foreign-source"] {
        let f = Fixture::with_ingress_authority("helper-source").await;
        let preparation = prepare(&f, "pinned").await;
        let observed = graph_admit(&f, &preparation).await;
        // Keep the proposed child, source row and publication association equal.
        // Only the authority differs from this helper source owner's ingress.
        let (proposal, child, fact) = helper_fixture_with_authority(
            &f, &preparation, &observed,
            principal(authority, PrincipalKind::ServiceAccount),
        ).await;
        assert!(matches!(
            f.bridge(&f.store).publish_prepared_helper_work(
                &f.snapshot, &proposal, child, fact.clone(),
                f.access(&f.context(f.now)), conditions(&f, &fact, &f.lease),
            ).await,
            Err(SetupError::InvalidRecord)
        ));
        assert!(f.store.get(proposal.key()).await.unwrap().is_none());
    }
}

#[tokio::test]
async fn helper_publication_refuses_foreign_owner_item_contributors_and_missing_source_row() {
    for fault in [
        "owner",
        "item",
        "contributors",
        "foreign-contributor",
        "request",
        "missing-row",
    ] {
        let f = Fixture::new().await;
        let preparation = prepare(&f, "pinned").await;
        let observation = graph_admit(&f, &preparation).await;
        let (proposal, child, mut fact) = helper_fixture(&f, &preparation, &observation).await;
        let mut conditions = conditions(&f, &fact, &f.lease);
        match fault {
            "owner" => fact.source.key.owner = "toolkit/helper-originals/foreign".into(),
            "item" => fact.item = WorkItemId::new("different-native-item").unwrap(),
            "contributors" => fact.contributors.clear(),
            "foreign-contributor" => {
                fact.contributors = vec![native_fixture::association(
                    principal("different-person", PrincipalKind::Human),
                    f.destination.clone(),
                    "foreign-work",
                    "foreign-event",
                )]
            }
            "request" => fact.admitted_request.title = "changed after admission".into(),
            "missing-row" => conditions
                .owners
                .retain(|owner| owner.key != fact.source.key),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                f.bridge(&f.store)
                    .publish_prepared_helper_work(
                        &f.snapshot,
                        &proposal,
                        child,
                        fact,
                        f.access(&f.context(f.now)),
                        conditions
                    )
                    .await,
                Err(SetupError::InvalidRecord)
            ),
            "{fault}"
        );
        assert!(f.store.get(proposal.key()).await.unwrap().is_none());
    }
}

struct Owner<'a> {
    fixture: &'a Fixture,
    fail_after_graph: AtomicBool,
    completes: AtomicUsize,
}
#[async_trait]
impl NativeSetupWorkOwner for Owner<'_> {
    type Retained = ();
    async fn observe(
        &self,
        preparation: &NativeWorkPreparation,
    ) -> Result<Option<NativeWorkObservation<()>>, SetupError> {
        let request = admitted_request(preparation);
        let service = self.fixture.backend.service();
        let Some(actual) = service
            .observe_admission(preparation.admission_key().clone(), request.clone())
            .await?
        else {
            return Ok(None);
        };
        let item = actual.item().ok_or(SetupError::InvalidRecord)?.id.clone();
        Ok(Some(NativeWorkObservation {
            realm: preparation.backend_pin().realm().to_string(),
            namespace: preparation.backend_pin().namespace().clone(),
            item,
            admission: service
                .admission_identity(preparation.admission_key().clone(), request.clone())?,
            admitted_request: request,
            retained: (),
        }))
    }
    async fn complete(
        &self,
        preparation: &NativeWorkPreparation,
        claim: &EffectLease,
        observed: Option<NativeWorkObservation<()>>,
    ) -> Result<NativeWorkCompletion<()>, SetupError> {
        assert_eq!(claim.kind, EffectClaimKind::Dispatch);
        self.completes.fetch_add(1, Ordering::SeqCst);
        let observed = match observed {
            Some(value) => value,
            None => graph_admit(self.fixture, preparation).await,
        };
        if self.fail_after_graph.swap(false, Ordering::SeqCst) {
            return Err(SetupError::Denied);
        }
        let (proposal, child, fact) = helper_fixture(self.fixture, preparation, &observed).await;
        let bridge = self.fixture.bridge(&self.fixture.store);
        let context = self.fixture.context(self.fixture.now);
        let published = if self.fixture.store.get(preparation.key()).await?.is_some() {
            bridge
                .load_prepared_work_exact(
                    &operation_id(self.fixture.snapshot.request())?,
                    child.original_work(),
                    self.fixture.access(&context),
                    &self.fixture.backend,
                )
                .await?
        } else {
            bridge
                .publish_prepared_helper_work(
                    &self.fixture.snapshot,
                    &proposal,
                    child,
                    fact.clone(),
                    self.fixture.access(&context),
                    conditions(self.fixture, &fact, claim),
                )
                .await?
        };
        Ok(NativeWorkCompletion {
            observation: observed,
            published,
        })
    }
}
fn options() -> WorkAdmissionStepOptions {
    WorkAdmissionStepOptions {
        worker: id("bounded-owner"),
        lease_ms: 30_000,
        owner_timeout_ms: 10_000,
        settlement_budget_ms: 5_000,
    }
}
async fn release_fixture_claim(f: &Fixture) {
    f.store
        .acknowledge_effect(
            &f.lease,
            EffectOutcome::Uncertain {
                evidence: f.policy.data.clone(),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn graph_commit_before_source_failure_recovers_same_item_before_settlement() {
    let f = Fixture::new().await;
    let preparation = prepare(&f, "one exact request").await;
    release_fixture_claim(&f).await;
    let owner = Owner {
        fixture: &f,
        fail_after_graph: AtomicBool::new(true),
        completes: AtomicUsize::new(0),
    };
    let bridge = f.bridge(&f.store);
    let context = f.context(f.now);
    assert!(matches!(
        bridge
            .step_native_work_admission(&preparation, &owner, Some(f.access(&context)), &options())
            .await
            .unwrap(),
        NativeWorkStep::RetrySafe
    ));
    assert!(matches!(
        bridge
            .step_native_work_admission(&preparation, &owner, Some(f.access(&context)), &options())
            .await,
        Err(SetupError::Denied)
    ));
    let first = owner.observe(&preparation).await.unwrap().unwrap().item;
    assert!(f.store.get(preparation.key()).await.unwrap().is_none());
    assert_eq!(
        f.store
            .get_effect(preparation.effect_key())
            .await
            .unwrap()
            .unwrap()
            .status,
        EffectStatus::Uncertain
    );
    assert!(matches!(
        bridge
            .step_native_work_admission(&preparation, &owner, Some(f.access(&context)), &options())
            .await
            .unwrap(),
        NativeWorkStep::RetrySafe
    ));
    let NativeWorkStep::Completed(done) = bridge
        .step_native_work_admission(&preparation, &owner, Some(f.access(&context)), &options())
        .await
        .unwrap()
    else {
        panic!("not complete")
    };
    assert_eq!(done.item, first);
    assert_eq!(owner.completes.load(Ordering::SeqCst), 2);
    assert_eq!(
        f.store
            .get_effect(preparation.effect_key())
            .await
            .unwrap()
            .unwrap()
            .status,
        EffectStatus::Succeeded
    );
    let NativeWorkStep::Completed(replayed) = bridge
        .step_native_work_admission_for_operation(
            &operation_id(f.snapshot.request()).unwrap(),
            &owner,
            None,
            &options(),
        )
        .await
        .unwrap()
    else {
        panic!("not replayed")
    };
    assert_eq!(replayed.item, first);
    assert_eq!(owner.completes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancellation_after_graph_commit_prevents_source_and_fresh_dispatch() {
    let f = Fixture::new().await;
    let preparation = prepare(&f, "cancel gap").await;
    release_fixture_claim(&f).await;
    let owner = Owner {
        fixture: &f,
        fail_after_graph: AtomicBool::new(true),
        completes: AtomicUsize::new(0),
    };
    let bridge = f.bridge(&f.store);
    let context = f.context(f.now);
    let _ = bridge
        .step_native_work_admission(&preparation, &owner, Some(f.access(&context)), &options())
        .await
        .unwrap();
    assert!(
        bridge
            .step_native_work_admission(&preparation, &owner, Some(f.access(&context)), &options())
            .await
            .is_err()
    );
    let item = owner.observe(&preparation).await.unwrap().unwrap().item;
    bridge
        .cancel(
            &operation_id(f.snapshot.request()).unwrap(),
            ContextReference {
                owner: id("input-owner"),
                id: id("cancel-gap"),
                revision: rev("1"),
                content_digest: ContentDigest::of_bytes(b"cancel"),
            },
            f.access(&context),
        )
        .await
        .unwrap();
    let _ = bridge
        .step_native_work_admission(&preparation, &owner, Some(f.access(&context)), &options())
        .await
        .unwrap();
    assert!(matches!(
        bridge
            .step_native_work_admission(&preparation, &owner, Some(f.access(&context)), &options())
            .await
            .unwrap(),
        NativeWorkStep::DispatchNotAuthorized
    ));
    assert_eq!(
        owner.observe(&preparation).await.unwrap().unwrap().item,
        item
    );
    assert_eq!(owner.completes.load(Ordering::SeqCst), 1);
    assert!(f.store.get(preparation.key()).await.unwrap().is_none());
}

struct StalledOwner<'a> {
    owner: Owner<'a>,
}
#[async_trait]
impl NativeSetupWorkOwner for StalledOwner<'_> {
    type Retained = ();
    async fn observe(
        &self,
        preparation: &NativeWorkPreparation,
    ) -> Result<Option<NativeWorkObservation<()>>, SetupError> {
        self.owner.observe(preparation).await
    }
    async fn complete(
        &self,
        _: &NativeWorkPreparation,
        claim: &EffectLease,
        observed: Option<NativeWorkObservation<()>>,
    ) -> Result<NativeWorkCompletion<()>, SetupError> {
        assert_eq!(claim.kind, EffectClaimKind::Dispatch);
        assert!(observed.is_some());
        self.owner.completes.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }
}

#[tokio::test]
async fn timed_out_completion_keeps_prior_native_commit_uncertain_and_never_not_applied() {
    let f = Fixture::new().await;
    let preparation = prepare(&f, "stalled source publication").await;
    // Existing claimed attempt reached the real graph but never source publish.
    let item = graph_admit(&f, &preparation).await.item;
    release_fixture_claim(&f).await;
    let owner = StalledOwner {
        owner: Owner {
            fixture: &f,
            fail_after_graph: AtomicBool::new(false),
            completes: AtomicUsize::new(0),
        },
    };
    let bridge = f.bridge(&f.store);
    let context = f.context(f.now);
    assert!(matches!(
        bridge
            .step_native_work_admission(&preparation, &owner, Some(f.access(&context)), &options())
            .await
            .unwrap(),
        NativeWorkStep::RetrySafe
    ));
    let bounded = WorkAdmissionStepOptions {
        worker: id("timeout-owner"),
        lease_ms: 1_000,
        owner_timeout_ms: 50,
        settlement_budget_ms: 500,
    };
    assert!(matches!(
        bridge
            .step_native_work_admission(&preparation, &owner, Some(f.access(&context)), &bounded)
            .await,
        Err(SetupError::WorkAdmission(SetupWorkAdmissionError::Timeout))
    ));
    let effect = f
        .store
        .get_effect(preparation.effect_key())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(effect.status, EffectStatus::Uncertain);
    assert!(matches!(
        effect.outcome,
        Some(EffectOutcome::Uncertain { .. })
    ));
    assert_eq!(owner.owner.completes.load(Ordering::SeqCst), 1);
    assert_eq!(
        owner.observe(&preparation).await.unwrap().unwrap().item,
        item
    );
    assert!(f.store.get(preparation.key()).await.unwrap().is_none());
}

struct FrozenFinalPreparer;
impl SetupWorkPreparer for FrozenFinalPreparer {
    fn prepare(
        &self,
        request: &ValidatedSetupRequest,
    ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
        let mut result = UnboundPreparer {
            title: "exact final native request",
            expires: u64::MAX,
        }
        .prepare(request)?;
        result.work_request.realm_id = Some(result.scope.realm().to_string());
        result.work_request.namespace = Some(result.scope.namespace().clone());
        // Explicit payload fixture, not an authenticated contributor binding.
        result
            .work_request
            .external_refs
            .push(meerkat_workgraph::ExternalWorkRef {
                kind: "test.final-source-bound-request".into(),
                id: "frozen-source-set".into(),
                url: None,
            });
        Ok(result)
    }
}
struct NeverPrepareOnRecovery;
impl SetupWorkPreparer for NeverPrepareOnRecovery {
    fn prepare(
        &self,
        _: &ValidatedSetupRequest,
    ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
        panic!("recovery must use the exact saved native request")
    }
}

#[tokio::test]
async fn exact_final_native_request_survives_graph_commit_and_reopen_without_source_row() {
    let f = Fixture::new().await;
    let bridge = f.bridge(&f.store);
    let context = f.context(f.now);
    let key = bridge.native_work_effect_key(&f.snapshot).await.unwrap();
    let preparation = bridge
        .prepare_native_work(
            &f.snapshot,
            f.access(&context),
            &FrozenFinalPreparer,
            &f.backend,
        )
        .await
        .unwrap();
    assert_eq!(preparation.effect_key(), &key);
    let first = graph_admit(&f, &preparation).await;
    assert!(f.store.get(preparation.key()).await.unwrap().is_none());
    let reopened = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
    let bridge = f.bridge(&reopened);
    let recovered = bridge
        .prepare_native_work(
            &f.snapshot,
            f.access(&context),
            &NeverPrepareOnRecovery,
            &f.backend,
        )
        .await
        .unwrap();
    assert_eq!(
        canonical_json(recovered.work_request()).unwrap(),
        canonical_json(preparation.work_request()).unwrap()
    );
    assert_eq!(recovered.admission_key(), preparation.admission_key());
    bridge
        .validate_native_work_recovery(&recovered, &f.backend)
        .await
        .unwrap();
    let observed = f
        .backend
        .service()
        .observe_admission(
            recovered.admission_key().clone(),
            recovered.work_request().clone(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(&observed, WorkAdmissionOutcome::Replayed(_)));
    assert_eq!(observed.item().unwrap().id, first.item);
}

#[tokio::test]
async fn private_recovery_requires_exact_effect_input_owner_and_backend() {
    let f = Fixture::new().await;
    let preparation = prepare(&f, "private recovery").await;
    let bridge = f.bridge(&f.store);
    bridge
        .validate_native_work_recovery(&preparation, &f.backend)
        .await
        .unwrap();
    let mut wrong_effect = preparation.clone();
    wrong_effect.effect.id = "another-effect".into();
    assert!(
        bridge
            .validate_native_work_recovery(&wrong_effect, &f.backend)
            .await
            .is_err()
    );
    let foreign =
        SetupStore::new_conditional(&f.store, SetupScope::new(id("foreign"), id("deployment")))
            .unwrap();
    assert!(foreign.native_work_effect_key(&f.snapshot).await.is_err());
    assert!(
        foreign
            .validate_native_work_recovery(&preparation, &f.backend)
            .await
            .is_err()
    );
    let other_backend = SetupWorkBackend::new(
        f.backend.service().clone(),
        f.backend.pin().provider().into(),
        id("replacement-generation"),
        &[f.backend.pin().durability().clone()],
    )
    .unwrap();
    assert!(
        bridge
            .validate_native_work_recovery(&preparation, &other_backend)
            .await
            .is_err()
    );
    let mut changed_input = preparation;
    changed_input.work.request.title = "different request".into();
    assert!(
        bridge
            .validate_native_work_recovery(&changed_input, &f.backend)
            .await
            .is_err()
    );
}

mod helper_results;

// The external-reference owner must not receive the exact input already owned
// by this acquisition. These are SQLite/WorkGraph owner protocol controls.
struct DeniedStepReferences(AtomicUsize);
impl ContextReferenceOwner for DeniedStepReferences {
    fn verify_access(
        &self,
        _: &ContextReference,
        _: &SetupRequestEvidence,
        _: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(RequestError::ContextUnavailable)
    }
}
struct StepInputPreparer<'a> {
    kind: &'a str,
    expires: u64,
}
impl SetupWorkPreparer for StepInputPreparer<'_> {
    fn prepare(
        &self,
        request: &ValidatedSetupRequest,
    ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
        let mut value = UnboundPreparer { title: "exact stored input", expires: self.expires }
            .prepare(request)?;
        if self.kind != "external" {
            value.input_reference = canonical_setup_input_reference(request).unwrap();
            match self.kind {
                "canonical" => {}
                "owner" => value.input_reference.owner = id("different-owner"),
                "id" => value.input_reference.id = id("different-input"),
                "revision" => value.input_reference.revision = rev("different-revision"),
                _ => panic!("unknown input control"),
            }
        }
        Ok(value)
    }
}

#[tokio::test]
async fn native_step_accepts_canonical_inline_input_with_denied_external_owner() {
    let f = Fixture::new().await;
    let bridge = f.bridge(&f.store);
    let context = f.context(f.now);
    let denied = DeniedStepReferences(AtomicUsize::new(0));
    let access = || SetupAccess { evidence: &f.authority, context: &context, references: &denied };
    let preparation = bridge.prepare_native_work(&f.snapshot, access(),
        &StepInputPreparer { kind: "canonical", expires: f.deadline }, &f.backend).await.unwrap();
    assert!(preparation.input_reference() == &canonical_setup_input_reference(f.snapshot.request()).unwrap());
    release_fixture_claim(&f).await;
    let owner = Owner { fixture: &f, fail_after_graph: AtomicBool::new(false), completes: AtomicUsize::new(0) };
    assert!(matches!(bridge.step_native_work_admission(&preparation, &owner, Some(access()), &options())
        .await.unwrap(), NativeWorkStep::RetrySafe));
    assert_eq!(owner.completes.load(Ordering::SeqCst), 0);
    let NativeWorkStep::Completed(done) = bridge
        .step_native_work_admission(&preparation, &owner, Some(access()), &options()).await.unwrap()
    else { panic!("canonical input did not complete the actual owner protocol") };
    let NativeWorkStep::Completed(replay) = bridge
        .step_native_work_admission(&preparation, &owner, Some(access()), &options()).await.unwrap()
    else { panic!("completed input did not replay") };
    assert_eq!(done.item, replay.item);
    assert_eq!(done.admission, replay.admission);
    assert_eq!(owner.completes.load(Ordering::SeqCst), 1);
    assert_eq!(denied.0.load(Ordering::SeqCst), 0);
    assert_eq!(f.store.get_effect(preparation.effect_key()).await.unwrap().unwrap().status, EffectStatus::Succeeded);
    assert!(f.store.get(preparation.key()).await.unwrap().is_some());
}

#[tokio::test]
async fn native_step_preserves_changed_descriptor_and_external_owner_refusal() {
    for kind in ["owner", "id", "revision", "external"] {
        let f = Fixture::new().await;
        let bridge = f.bridge(&f.store);
        let context = f.context(f.now);
        // The external fixture owner permitted this exact preparation earlier.
        // Revoking that owner must prevent the next Dispatch claim; the
        // initial uncertain fixture claim is reconciled without fresh authority.
        let preparation = bridge.prepare_native_work(&f.snapshot, f.access(&context),
            &StepInputPreparer { kind, expires: f.deadline }, &f.backend).await.unwrap();
        release_fixture_claim(&f).await;
        let owner = Owner { fixture: &f, fail_after_graph: AtomicBool::new(false), completes: AtomicUsize::new(0) };
        let denied = DeniedStepReferences(AtomicUsize::new(0));
        let access = || SetupAccess { evidence: &f.authority, context: &context, references: &denied };
        assert!(matches!(bridge.step_native_work_admission(&preparation, &owner, Some(access()), &options())
            .await.unwrap(), NativeWorkStep::RetrySafe));
        let before = f.store.get_effect(preparation.effect_key()).await.unwrap().unwrap();
        assert!(matches!(bridge.step_native_work_admission(&preparation, &owner, Some(access()), &options()).await,
            Err(SetupError::Request(RequestError::ContextUnavailable))), "{kind}");
        assert_eq!(denied.0.load(Ordering::SeqCst), 1, "{kind}");
        assert_eq!(owner.completes.load(Ordering::SeqCst), 0, "{kind}");
        assert_eq!(f.store.get_effect(preparation.effect_key()).await.unwrap().unwrap(), before);
        assert!(owner.observe(&preparation).await.unwrap().is_none());
        assert!(f.store.get(preparation.key()).await.unwrap().is_none());
    }
}

#[tokio::test]
async fn native_step_rejects_changed_canonical_content_before_owner_completion() {
    let f = Fixture::new().await;
    let bridge = f.bridge(&f.store);
    let context = f.context(f.now);
    let denied = DeniedStepReferences(AtomicUsize::new(0));
    let access = || SetupAccess { evidence: &f.authority, context: &context, references: &denied };
    let mut preparation = bridge.prepare_native_work(&f.snapshot, access(),
        &StepInputPreparer { kind: "canonical", expires: f.deadline }, &f.backend).await.unwrap();
    // Corrupt only the held descriptor. The exact stored input remains intact.
    preparation.work.input_reference.content_digest = ContentDigest::of_bytes(b"changed content");
    let owner = Owner { fixture: &f, fail_after_graph: AtomicBool::new(false), completes: AtomicUsize::new(0) };
    let before = f.store.get_effect(preparation.effect_key()).await.unwrap().unwrap();
    assert!(matches!(bridge.step_native_work_admission(&preparation, &owner, Some(access()), &options()).await,
        Err(SetupError::InvalidRecord)));
    assert_eq!(denied.0.load(Ordering::SeqCst), 0);
    assert_eq!(owner.completes.load(Ordering::SeqCst), 0);
    assert_eq!(f.store.get_effect(preparation.effect_key()).await.unwrap().unwrap(), before);
    assert!(owner.observe(&preparation).await.unwrap().is_none());
    assert!(f.store.get(preparation.key()).await.unwrap().is_none());
}


#[tokio::test]
async fn retained_input_and_final_publication_use_distinct_local_admissions() {
    let f = Fixture::new().await;
    let preparation = prepare(&f, "retained input survives publication").await;
    let input_key = preparation.input_record.as_ref().unwrap().key.clone();
    let input = f.store.get(&input_key).await.unwrap().unwrap();
    let native_key = preparation.admission_key().clone();
    let effect = f.store.get_effect(preparation.effect_key()).await.unwrap().unwrap();
    let observed = graph_admit(&f, &preparation).await;
    let (proposal, child, fact) = helper_fixture(&f, &preparation, &observed).await;
    assert_eq!(input.key.realm, proposal.key.realm);
    assert_eq!(input.key.owner, proposal.key.owner);
    assert_ne!(input.key.id, proposal.key.id);
    assert_eq!(proposal.admission_key, native_key.as_str());
    assert_ne!(
        publication_admission_key(&proposal.key, &proposal.admission_key).unwrap(),
        native_key.as_str()
    );
    // The input already reserves the historical raw native key in this owner.
    assert!(matches!(
        f.store.admit(AdmissionRequest {
            key: input.key.clone(),
            admission_key: native_key.as_str().to_owned(),
            data: input.data.clone(),
            events: vec![],
            effects: vec![],
        }).await.unwrap(),
        AdmissionOutcome::Existing(_)
    ));
    let bridge = f.bridge(&f.store);
    let context = f.context(f.now);
    let published = bridge.publish_prepared_helper_work(
        &f.snapshot, &proposal, child.clone(), fact.clone(),
        f.access(&context), conditions(&f, &fact, &f.lease),
    ).await.unwrap();
    let saved = f.store.get(published.key()).await.unwrap().unwrap();
    let replay = bridge.publish_prepared_helper_work(
        &f.snapshot, &proposal, child, fact.clone(),
        f.access(&context), conditions(&f, &fact, &f.lease),
    ).await.unwrap();
    assert_eq!(published.key(), replay.key());
    assert_eq!(published.digest(), replay.digest());
    assert_eq!(f.store.get(published.key()).await.unwrap().unwrap(), saved);
    assert_eq!(f.store.get(&input_key).await.unwrap().unwrap(), input);
    assert_eq!(preparation.admission_key(), &native_key);
    assert_eq!(published.helper_source().unwrap().admission.key, native_key);
    assert_eq!(f.store.get_effect(preparation.effect_key()).await.unwrap().unwrap(), effect);
    let current = f.backend.service().observe_admission(
        preparation.admission_key().clone(), admitted_request(&preparation),
    ).await.unwrap().unwrap();
    assert_eq!(current.item().unwrap().id, observed.item);
}

#[tokio::test]
async fn legacy_raw_key_final_publication_replays_without_new_admission() {
    let f = Fixture::new().await;
    let proposal = f.propose("legacy final publication").await;
    let child = f.child_evidence(&proposal, f.now);
    let mut owners = f.conditions().owners;
    owners.push(RecordCondition {
        key: f.snapshot.key().clone(),
        revision: f.snapshot.stored.revision,
        content_digest: f.snapshot.stored.content_digest.clone(),
        claim: None,
        dispatch_claim: Some(f.lease.clone()),
    });
    // Exact historical v4 record format and its old local admission identity.
    let record = NativePreparedRecord {
        work: proposal.work.clone(),
        metadata: proposal.metadata.clone(),
        delegation: proposal.delegation.clone(),
        child: child.clone(),
        helper_source: None,
        basis: publication_basis(&owners, proposal.key()).unwrap(),
    };
    let legacy_request = AdmissionRequest {
        key: proposal.key.clone(),
        admission_key: proposal.admission_key.clone(),
        data: RecordData {
            schema: SCHEMA.into(), version: VERSION,
            body: serde_json::to_value(record).unwrap(),
        },
        events: vec![], effects: vec![],
    };
    let saved = f.store.admit(legacy_request).await.unwrap().record().clone();
    let reopened = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
    let mut prior_conditions = f.conditions();
    prior_conditions.before_ms = 1;
    let replay = f.bridge(&reopened).publish_prepared_work(
        &f.snapshot, &proposal, child.clone(), f.access(&f.context(f.now)),
        prior_conditions,
    ).await.unwrap();
    assert_eq!(replay.key(), &saved.key);
    assert_eq!(replay.digest().to_string(), saved.content_digest);
    assert_eq!(replay.child(), &child);
    assert_eq!(reopened.get(&saved.key).await.unwrap().unwrap(), saved);
    // The historical pre-read did not reserve the successor admission key.
    let mut probe_key = proposal.key.clone();
    probe_key.id = "new-key-remains-unused".into();
    assert!(matches!(
        reopened.admit(AdmissionRequest {
            key: probe_key,
            admission_key: publication_admission_key(&proposal.key, &proposal.admission_key).unwrap(),
            data: f.policy.data.clone(), events: vec![], effects: vec![],
        }).await.unwrap(),
        AdmissionOutcome::Created(_)
    ));
}

#[tokio::test]
async fn unrelated_publication_admission_conflict_is_not_missing_record() {
    let f = Fixture::new().await;
    let proposal = f.propose("conflicting local admission").await;
    let mut foreign_key = proposal.key.clone();
    foreign_key.id = "different-publication".into();
    let foreign = f.store.admit(AdmissionRequest {
        key: foreign_key,
        admission_key: publication_admission_key(&proposal.key, &proposal.admission_key).unwrap(),
        data: f.policy.data.clone(), events: vec![], effects: vec![],
    }).await.unwrap().record().clone();
    let effect = f.store.get_effect(&f.lease.key).await.unwrap().unwrap();
    assert!(matches!(
        f.bridge(&f.store).publish_prepared_work(
            &f.snapshot, &proposal, f.child_evidence(&proposal, f.now),
            f.access(&f.context(f.now)), f.conditions(),
        ).await,
        Err(SetupError::Store(StoreError::AdmissionConflict))
    ));
    assert!(f.store.get(proposal.key()).await.unwrap().is_none());
    assert_eq!(f.store.get(&foreign.key).await.unwrap().unwrap(), foreign);
    assert_eq!(f.store.get_effect(&f.lease.key).await.unwrap().unwrap(), effect);
}

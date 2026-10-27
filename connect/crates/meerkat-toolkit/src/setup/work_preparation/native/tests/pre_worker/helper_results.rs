//! Canonical result storage and replay using the existing SQLite/WorkGraph
//! fixture. Native operation authorization deliberately denies in this fixture;
//! these controls do not qualify an installed helper or native delivery.
use super::*;
use crate::setup::{SetupHelperFailure, SetupHelperResultAccess};

async fn published_helper(
    f: &Fixture,
) -> (
    NativeWorkPreparation,
    SetupRequestEvidence,
    NativeHelperSource,
    Vec<(InputAuthorityAssociation, RecordCondition)>,
) {
    let preparation = prepare(f, "canonical helper input").await;
    let observed = graph_admit(f, &preparation).await;
    let (proposal, child, source) = helper_fixture(f, &preparation, &observed).await;
    f.bridge(&f.store)
        .publish_prepared_helper_work(
            &f.snapshot,
            &proposal,
            child.clone(),
            source.clone(),
            f.access(&f.context(f.now)),
            conditions(f, &source, &f.lease),
        )
        .await
        .unwrap();
    // Explicit source history remains fixture data, not native permission.
    let original = f.store.admit(AdmissionRequest {
        key: StoreKey { realm: "tenant".into(), owner: "fixture-original".into(), id: "result-parent".into() },
        admission_key: "result-parent".into(),
        data: RecordData { schema: "test.original_source".into(), version: 1,
            body: serde_json::to_value(f.authority.association()).unwrap() },
        events: vec![], effects: vec![],
    }).await.unwrap().record().clone();
    let contributors = vec![(f.authority.association().clone(), RecordCondition {
        key: original.key, revision: original.revision, content_digest: original.content_digest,
        claim: None, dispatch_claim: None,
    })];
    (preparation, child, source, contributors)
}
fn helper_context<'a>(
    child: &'a SetupRequestEvidence,
    authorization: &'a WorkAuthorizationContext,
    now_ms: u64,
) -> SetupUseContext<'a> {
    let c = child.constraints();
    SetupUseContext {
        tenant: &c.tenant,
        deployment: &c.deployment,
        realm: &child.association().candidate().ingress_namespace.realm,
        target: &child.association().candidate().target,
        policy_revision: &c.policy_revision,
        now_ms,
        authorization,
    }
}
fn access<'a>(
    source: &'a NativeHelperSource,
    input: &'a ContextReference,
    context: &'a SetupUseContext<'a>,
    contributor_sources: &'a [(InputAuthorityAssociation, RecordCondition)],
) -> SetupHelperResultAccess<'a> {
    SetupHelperResultAccess {
        helper: &source.association,
        source: &source.source,
        contributors: &source.contributors,
        contributor_sources,
        ancestor_sources: &[],
        effect: &source.effect,
        input_digest: &input.content_digest,
        context,
    }
}
fn report() -> SetupHelperFailure {
    SetupHelperFailure::Unavailable {
        detail: "This provider does not offer the requested export operation.".into(),
    }
}

#[tokio::test]
async fn canonical_helper_result_survives_lost_reply_and_binds_explanation_before_delivery() {
    let f = Fixture::new().await;
    let (preparation, child, source, contributor_sources) = published_helper(&f).await;
    let operation = operation_id(f.snapshot.request()).unwrap();
    let authorization = native_fixture::current(child.association());
    let context = helper_context(&child, &authorization, f.now);
    let before = f.store.get(f.snapshot.key()).await.unwrap().unwrap();
    let initial_page = f.bridge(&f.store).continuation_recovery_page(None, 32).await.unwrap();
    assert!(initial_page.operations.is_empty());
    assert!(!initial_page.had_unavailable_records);
    assert!(before.data.body.get("helper_detail").is_none());
    let legacy = f.bridge(&f.store).recover(before.clone()).unwrap();
    assert_eq!(
        canonical_json(&legacy.record).unwrap(),
        canonical_json(&before.data.body).unwrap()
    );
    for detail in [String::new(), " ".into(), "x".repeat(4097)] {
        assert!(
            f.bridge(&f.store)
                .resolve_helper_failure(
                    &operation,
                    SetupHelperFailure::Failed { detail },
                    access(&source, preparation.input_reference(), &context, &contributor_sources),
                    &f.backend,
                    "invalid-detail"
                )
                .await
                .is_err()
        );
    }
    let first = f
        .bridge(&f.store)
        .resolve_helper_failure(
            &operation,
            report(),
            access(&source, preparation.input_reference(), &context, &contributor_sources),
            &f.backend,
            "result-fixture",
        )
        .await
        .unwrap();
    assert_eq!(first.phase, AcquisitionPhase::Unavailable);
    // No continuation owner has run: the result and handoff already coexist in
    // canonical storage and survive closing/reopening its real SQLite adapter.
    let reopened = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
    let bridge = f.bridge(&reopened);
    // Recovery selects the saved obligation without another helper call. The
    // cursor is exact scoped pagination, not a delivery or restart checkpoint.
    let page = bridge.continuation_recovery_page(None, 1).await.unwrap();
    assert_eq!(page.operations, vec![operation.clone()]);
    assert!(!page.had_unavailable_records);
    assert_eq!(page.next_after_id.as_deref(), Some(f.snapshot.key().id.as_str()));
    let next = bridge.continuation_recovery_page(page.next_after_id.as_deref(), 1).await.unwrap();
    assert!(next.operations.is_empty());
    assert!(next.next_after_id.is_none());
    for limit in [0, 65] {
        assert!(bridge.continuation_recovery_page(None, limit).await.is_err());
    }
    for scope in [SetupScope::new(id("foreign"), id("deployment")), SetupScope::new(id("tenant"), id("foreign"))] {
        let other = SetupStore::new_conditional(&reopened, scope).unwrap();
        let page = other.continuation_recovery_page(None, 32).await.unwrap();
        assert!(page.operations.is_empty());
        assert!(!page.had_unavailable_records);
    }
    let obligation = bridge
        .committed_continuation(&operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        obligation.helper_detail(),
        Some("This provider does not offer the requested export operation.")
    );
    assert_eq!(
        obligation.continuation().audience,
        f.snapshot.request().host().continuation.audience
    );
    assert_eq!(obligation.original_work(), f.authority.original_work());
    let saved = reopened.get(f.snapshot.key()).await.unwrap().unwrap();
    let record = bridge.recover(saved.clone()).unwrap();
    assert!(record.record.continuation_receipt.is_none());
    let (expected_events, expected_effects) = handoffs(
        &record.record,
        vec![AcquisitionEffect::ContinuationRequested {
            operation_id: record.state().operation_id.clone(),
            continuation_id: record.state().continuation_id.clone(),
            terminal_revision: record.state().terminal_revision,
            outcome: record.state().lifecycle_phase,
        }],
    )
    .unwrap();
    assert_eq!(expected_events.len(), 1);
    assert_eq!(expected_effects.len(), 1);
    let effect = reopened
        .get_effect(&EffectKey {
            record: saved.key.clone(),
            id: expected_effects[0].id.clone(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(effect.intent, expected_effects[0]);
    let replay = bridge
        .resolve_helper_failure(
            &operation,
            report(),
            access(&source, preparation.input_reference(), &context, &contributor_sources),
            &f.backend,
            "replay-fixture",
        )
        .await
        .unwrap();
    assert_eq!(first.revision, replay.revision);
    assert_eq!(
        saved.content_digest,
        reopened
            .get(&saved.key)
            .await
            .unwrap()
            .unwrap()
            .content_digest
    );
    assert!(matches!(
        bridge
            .resolve_helper_failure(
                &operation,
                SetupHelperFailure::Failed {
                    detail: "Different result under the same acquisition.".into()
                },
                access(&source, preparation.input_reference(), &context, &contributor_sources),
                &f.backend,
                "conflicting-fixture"
            )
            .await,
        Err(SetupError::Store(StoreError::AdmissionConflict))
    ));
    // Changing the human explanation changes exact obligation content, while
    // the original task's delivery key remains the same and must conflict.
    let mut altered = obligation.clone();
    altered.helper_detail = Some("substituted content".into());
    assert_ne!(obligation.digest().unwrap(), altered.digest().unwrap());
    assert_eq!(
        obligation.delivery_identity().unwrap(),
        altered.delivery_identity().unwrap()
    );
}

#[tokio::test]
async fn helper_result_refuses_substituted_source_effect_parent_and_current_context() {
    let f = Fixture::new().await;
    let (preparation, child, source, contributor_sources) = published_helper(&f).await;
    let operation = operation_id(f.snapshot.request()).unwrap();
    let authorization = native_fixture::current(child.association());
    let context = helper_context(&child, &authorization, f.now);
    for kind in [
        "helper",
        "effect",
        "acquisition",
        "source",
        "parent",
        "input",
    ] {
        let mut changed = source.clone();
        let mut input = preparation.input_reference().clone();
        match kind {
            "helper" => changed.association = f.authority.association().clone(),
            "effect" => changed.effect.id.push_str("-foreign"),
            "acquisition" => changed.effect.record.id.push_str("-foreign"),
            "source" => changed.source.key.id.push_str("-foreign"),
            "parent" => changed.contributors = vec![child.association().clone()],
            "input" => input.content_digest = ContentDigest::of_bytes(b"other input"),
            _ => unreachable!(),
        }
        assert!(
            f.bridge(&f.store)
                .resolve_helper_failure(
                    &operation,
                    report(),
                    access(&changed, &input, &context, &contributor_sources),
                    &f.backend,
                    "negative-fixture"
                )
                .await
                .is_err(),
            "{kind}"
        );
    }
    let wrong_context = helper_context(&child, &f.authorization, f.now);
    assert!(
        f.bridge(&f.store)
            .resolve_helper_failure(
                &operation,
                report(),
                access(&source, preparation.input_reference(), &wrong_context, &contributor_sources),
                &f.backend,
                "parent-context"
            )
            .await
            .is_err()
    );
    let mut stale = context;
    let old_revision = rev("replaced-policy");
    stale.policy_revision = &old_revision;
    assert!(
        f.bridge(&f.store)
            .resolve_helper_failure(
                &operation,
                report(),
                access(&source, preparation.input_reference(), &stale, &contributor_sources),
                &f.backend,
                "stale-policy"
            )
            .await
            .is_err()
    );
    assert!(
        f.bridge(&f.store)
            .committed_continuation(&operation)
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
fn helper_report_shape_cannot_assert_ready_or_smuggle_authority() {
    for value in [
        serde_json::json!({"kind":"ready","detail":"done"}),
        serde_json::json!({"kind":"failed","detail":"No export","operation":"other"}),
        serde_json::json!({"kind":"failed","detail":"No export","authority":{}}),
    ] {
        assert!(serde_json::from_value::<SetupHelperFailure>(value).is_err());
    }
}


#[tokio::test]
async fn helper_result_uses_current_owner_condition_without_rewriting_publication() {
    let f = Fixture::new().await;
    let (preparation, child, source, contributor_sources) = published_helper(&f).await;
    let operation = operation_id(f.snapshot.request()).unwrap();
    let authorization = native_fixture::current(child.association());
    let context = helper_context(&child, &authorization, f.now);
    let before = f.store.get(&source.source.key).await.unwrap().unwrap();
    // This Toolkit-layer fixture has opaque source data. The installed source
    // owner's real typed-input checks are covered by the native helper test.
    let advanced = f.store.compare_and_swap(toolkit_store::Transition {
        key: before.key.clone(), expected_revision: before.revision,
        expected_digest: Some(before.content_digest.clone()), data: before.data,
        claim: None, events: vec![], effects: vec![],
    }).await.unwrap();
    let mut current = source.clone();
    current.source.revision = advanced.revision;
    current.source.content_digest = advanced.content_digest.clone();
    let (_, published) = f.bridge(&f.store).helper_result_snapshot(&operation,
        &access(&current, preparation.input_reference(), &context, &contributor_sources), &f.backend).await.unwrap();
    assert_eq!(published.helper_source().unwrap().source, source.source);
    for kind in ["stale", "regressed", "ahead", "digest", "claim", "dispatch"] {
        let mut changed = current.clone();
        match kind {
            "stale" => changed.source = source.source.clone(),
            "regressed" => changed.source.revision = source.source.revision - 1,
            "ahead" => changed.source.revision += 1,
            "digest" => {
                changed.source.revision = source.source.revision;
                changed.source.content_digest = "changed-content".into();
            }
            "claim" => changed.source.claim = Some(toolkit_store::RecordLease {
                store_id: "foreign".into(), key: changed.source.key.clone(),
                worker: "foreign".into(), fence: 1, expires_at_ms: 1,
            }),
            "dispatch" => changed.source.dispatch_claim = Some(f.lease.clone()),
            _ => unreachable!(),
        }
        assert!(f.bridge(&f.store).helper_result_snapshot(&operation,
            &access(&changed, preparation.input_reference(), &context, &contributor_sources), &f.backend).await.is_err(), "{kind}");
    }
    let (_, again) = f.bridge(&f.store).helper_result_snapshot(&operation,
        &access(&current, preparation.input_reference(), &context, &contributor_sources), &f.backend).await.unwrap();
    assert_eq!(again.helper_source().unwrap().source, source.source);
}

// The peer mutation occurs after the producer's final reads, immediately
// before the actual SQLite transaction. Intercept the old unconditional path
// too, so a result-only CAS cannot make this regression pass accidentally.
struct ResultBoundaryStore<'a> {
    inner: &'a SqliteStore,
    target: StoreKey,
    source: StoredRecord,
    fired: AtomicBool,
}

impl ResultBoundaryStore<'_> {
    async fn mutate_before_commit(&self, key: &StoreKey) {
        if key != &self.target || self.fired.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut data = self.source.data.clone();
        data.body["changed_at_result_commit"] = serde_json::Value::Bool(true);
        self.inner.compare_and_swap(Transition {
            key: self.source.key.clone(),
            expected_revision: self.source.revision,
            expected_digest: Some(self.source.content_digest.clone()),
            claim: None, data, events: vec![], effects: vec![],
        }).await.unwrap();
    }
}

#[async_trait::async_trait]
impl ConditionalAdmissionStore for ResultBoundaryStore<'_> {
    async fn admit_conditionally(&self, request: ConditionalAdmission) -> StoreResult<AdmissionOutcome> {
        self.inner.admit_conditionally(request).await
    }
    async fn compare_and_swap_conditionally(&self, request: ConditionalTransition) -> StoreResult<StoredRecord> {
        self.mutate_before_commit(&request.transition.key).await;
        self.inner.compare_and_swap_conditionally(request).await
    }
}

#[tokio::test]
async fn helper_result_fences_contributor_and_helper_changes_at_commit() {
    for changed in ["contributor", "helper", "ancestor"] {
        let f = Fixture::new().await;
        let (preparation, child, mut source, mut contributors) = published_helper(&f).await;
        let ancestor = f.store.admit(AdmissionRequest {
            key: StoreKey { realm: "tenant".into(), owner: "fixture-ancestor".into(), id: "approved-parent".into() },
            admission_key: "approved-parent".into(),
            data: RecordData { schema: "test.ancestor_source".into(), version: 1,
                body: serde_json::json!({"cancelled": false}) }, events: vec![], effects: vec![],
        }).await.unwrap().record().clone();
        let mut ancestors = vec![RecordCondition {
            key: ancestor.key, revision: ancestor.revision, content_digest: ancestor.content_digest,
            claim: None, dispatch_claim: None,
        }];
        let source_key = match changed {
            "contributor" => &contributors[0].1.key,
            "helper" => &source.source.key,
            "ancestor" => &ancestors[0].key,
            _ => unreachable!(),
        };
        let raced = ResultBoundaryStore {
            inner: &f.store,
            target: f.snapshot.key().clone(),
            source: f.store.get(source_key).await.unwrap().unwrap(),
            fired: AtomicBool::new(false),
        };
        let operation = operation_id(f.snapshot.request()).unwrap();
        let authorization = native_fixture::current(child.association());
        let context = helper_context(&child, &authorization, f.now);
        let bridge = SetupStore::new_conditional(
            &raced, SetupScope::new(id("tenant"), id("deployment")),
        ).unwrap();
        let before = f.store.get(f.snapshot.key()).await.unwrap().unwrap();
        let effects = f.store.list_effects(f.snapshot.key(), None, 64).await.unwrap();
        let mut result_access = access(&source, preparation.input_reference(), &context, &contributors);
        result_access.ancestor_sources = &ancestors;
        let result = bridge.resolve_helper_failure(
            &operation, report(), result_access,
            &f.backend, "ordinary-result-race",
        ).await;
        assert!(raced.fired.load(Ordering::SeqCst), "{changed}");
        assert!(matches!(result, Err(SetupError::Store(StoreError::Conflict))), "{changed}");
        assert_eq!(f.store.get(f.snapshot.key()).await.unwrap().unwrap(), before, "{changed}");
        assert_eq!(f.store.list_effects(f.snapshot.key(), None, 64).await.unwrap(), effects, "{changed}");
        assert!(bridge.committed_continuation(&operation).await.unwrap().is_none(), "{changed}");

        // Only source revision changed in this storage fixture. A fresh read
        // retains the same helper publication and exact contributor identity.
        let current = f.store.get(&raced.source.key).await.unwrap().unwrap();
        let condition = RecordCondition {
            key: current.key, revision: current.revision, content_digest: current.content_digest,
            claim: None, dispatch_claim: None,
        };
        match changed {
            "contributor" => contributors[0].1 = condition,
            "helper" => source.source = condition,
            "ancestor" => ancestors[0] = condition,
            _ => unreachable!(),
        }
        let mut current_access = access(&source, preparation.input_reference(), &context, &contributors);
        current_access.ancestor_sources = &ancestors;
        let committed = f.bridge(&f.store).resolve_helper_failure(
            &operation, report(), current_access,
            &f.backend, "ordinary-result-after-race",
        ).await.unwrap();
        assert_eq!(committed.phase, AcquisitionPhase::Unavailable, "{changed}");
        let obligation = f.bridge(&f.store).committed_continuation(&operation).await.unwrap().unwrap();
        assert_eq!(obligation.original_work(), f.authority.original_work(), "{changed}");
    }
}

#[async_trait::async_trait]
impl ToolkitStore for ResultBoundaryStore<'_> {
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
        self.inner.get(key).await
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
        self.mutate_before_commit(&transition.key).await;
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
    async fn recover_record_claim(&self, lease: &RecordLease) -> StoreResult<RecordLease> {
        self.inner.recover_record_claim(lease).await
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

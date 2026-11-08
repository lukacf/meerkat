//! Store/owner protocol controls with real SQLite and WorkGraph create, claim
//! and Parent-link operations. The existing request and continuation owners
//! are fixtures. Controller observations below are mechanical protocol input,
//! not checked-install authority, native admission, SDK launch or Ready proof.

use super::*;
use meerkat_core::MobMemberBinding;
use meerkat_workgraph::{
    ClaimWorkItemRequest, CreateWorkItemRequest, LinkWorkItemsRequest, SqliteWorkGraphStore,
    WorkEdgeKind, WorkGraphService, WorkItem, WorkItemRef, WorkNamespace, WorkOwner, WorkOwnerKey,
};
use serde_json::{Value, json};
use std::sync::Arc;
use toolkit_package::canonical_json;

const CYCLE: &str = "shared-protocol-cycle";

async fn claimed(service: &WorkGraphService, name: &str) -> WorkItem {
    let item = service
        .create(CreateWorkItemRequest {
            title: name.into(),
            labels: [CYCLE.into()].into(),
            ..Default::default()
        })
        .await
        .unwrap();
    service
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
        .unwrap()
}

fn work_ref(item: &WorkItem) -> WorkItemRef {
    WorkItemRef {
        realm_id: item.realm_id.clone(),
        namespace: item.namespace.clone(),
        item_id: item.id.clone(),
    }
}

pub(super) async fn backend(f: &Fixture) -> (SetupWorkBackend, WorkItem) {
    let service = WorkGraphService::with_scope(
        Arc::new(SqliteWorkGraphStore::open(f.directory.path().join("work.sqlite")).unwrap()),
        "tenant-a",
        WorkNamespace::new("shared-setup").unwrap(),
    );
    let parent = claimed(&service, "parent").await;
    let backend = SetupWorkBackend::new(
        service,
        "fixture-sqlite".into(),
        id("shared-workgraph-generation"),
        &[meerkat_core::DurabilityDeclaration::durable(
            "workgraph",
            meerkat_core::DurabilityResolution::Persistent,
        )],
    )
    .unwrap();
    (backend, parent)
}

pub(super) async fn acquisition(
    f: &mut Fixture,
    backend: &SetupWorkBackend,
    parent: &WorkItem,
    name: &str,
) -> SetupSnapshot {
    let child = claimed(backend.service(), name).await;
    backend
        .service()
        .link(LinkWorkItemsRequest {
            realm_id: None,
            namespace: None,
            kind: WorkEdgeKind::Parent,
            from_id: child.id.clone(),
            to_id: parent.id.clone(),
        })
        .await
        .unwrap();
    let work = SetupWorkContext::from_owner(
        backend.pin().clone(),
        work_ref(parent),
        work_ref(&child),
        parent.owner.as_ref().unwrap().key.clone(),
        child.owner.as_ref().unwrap().key.clone(),
        CYCLE.into(),
    )
    .unwrap();
    work.check_current(backend).await.unwrap();
    let mut host = f.request.host().clone();
    host.correlation = id(name);
    host.continuation.delivery_key = id(&format!("delivery-{name}"));
    host.work_context = Some(work);
    host.setup_helper = Some(MobMemberBinding {
        mob_id: "protocol-host".into(),
        role: "setup".into(),
        member: format!("helper-{name}"),
    });
    f.request = validate_request(
        f.request.request().clone(),
        host,
        &f.authority,
        &f.context(200),
        &f.references,
    )
    .unwrap();
    f.admit().await
}

fn selection(account: &str, context: &str) -> SharedPreparationSelection {
    let request = json!({"profile": "python", "package": "protocol-example", "account": account,
        "context": {"reference": context, "revision": "1"}});
    let arguments = json!({"request": request});
    SharedPreparationSelection::from_controller(json!({
        "controller": "a".repeat(64),
        "arguments_sha256": ContentDigest::of_bytes(&canonical_json(&arguments).unwrap()).to_string(),
        "arguments": arguments,
        "effect": {"request": request},
        "plan": {"artifact": "same-artifact", "digest": "b".repeat(64)},
        "result": {"validation_id": "c".repeat(32)}
    }))
    .unwrap()
}

fn completion() -> SharedPreparationCompletion {
    SharedPreparationCompletion::from_controller(json!({
        "controller": "a".repeat(64), "validation_id": "c".repeat(32),
        "job": {"id": "d".repeat(32), "kind": "install", "status": "succeeded",
            "family": "selected-python", "result": {"artifact": "same-artifact"}, "error": null}
    }))
    .unwrap()
}

pub(super) fn member(
    f: &Fixture,
    snapshot: &SetupSnapshot,
    selected: &SharedPreparationSelection,
) -> SharedPreparationMember {
    f.bridge()
        .shared_preparation_member(
            snapshot,
            StoreKey {
                realm: snapshot.key().realm.clone(),
                owner: "protocol-source".into(),
                id: snapshot.key().id.clone(),
            },
            selected,
        )
        .unwrap()
}

async fn row(f: &Fixture, member: &SharedPreparationMember) -> StoredRecord {
    f.store.get(member.key()).await.unwrap().unwrap()
}

fn stored_member<'a>(record: &'a StoredRecord, member: &SharedPreparationMember) -> &'a Value {
    record.data.body["members"]
        .as_object()
        .unwrap()
        .values()
        .find(|value| value["acquisition"] == serde_json::to_value(member.acquisition()).unwrap())
        .unwrap()
}

#[tokio::test]
async fn physical_completion_allows_exact_late_member_but_last_leave_seals_group() {
    let mut f = Fixture::new();
    let (backend, parent) = backend(&f).await;
    let selected = selection("account-a", "context-a");
    let a = acquisition(&mut f, &backend, &parent, "child-a").await;
    let b = acquisition(&mut f, &backend, &parent, "child-b").await;
    let c = acquisition(&mut f, &backend, &parent, "child-c").await;
    let a = member(&f, &a, &selected);
    let b = member(&f, &b, &selected);
    let c = member(&f, &c, &selected);
    assert!(f.bridge().shared_preparation_selection(&a).await.is_err());
    assert_eq!(a.key(), b.key());
    assert_ne!(a.acquisition(), b.acquisition());
    assert_ne!(a.source(), b.source());
    f.bridge()
        .join_shared_preparation(&a, &selected, &backend)
        .await
        .unwrap();
    f.bridge()
        .complete_shared_preparation(&a, &completion(), &backend)
        .await
        .unwrap();
    let completed = row(&f, &a).await;
    assert!(f.bridge().shared_preparation_selection(&a).await.unwrap() == selected);
    assert!(
        f.bridge()
            .shared_preparation_left_selection(&a)
            .await
            .is_err()
    );
    assert!(f.bridge().shared_preparation_selection(&c).await.is_err());
    assert_eq!(
        row(&f, &a).await,
        completed,
        "selection does not join a missing member"
    );
    assert!(!completed.data.body["completion"].is_null());
    assert!(stored_member(&completed, &a)["delivered"].is_null());

    // The late member joins the same physical result, with its own source and
    // no delivery fact. Re-observation cannot rewrite that immutable result.
    f.bridge()
        .join_shared_preparation(&b, &selected, &backend)
        .await
        .unwrap();
    let joined = row(&f, &a).await;
    assert_eq!(joined.data.body["members"].as_object().unwrap().len(), 2);
    assert_eq!(
        joined.data.body["completion"],
        completed.data.body["completion"]
    );
    assert_eq!(stored_member(&joined, &a), stored_member(&completed, &a));
    assert!(stored_member(&joined, &b)["delivered"].is_null());
    f.bridge()
        .complete_shared_preparation(&b, &completion(), &backend)
        .await
        .unwrap();
    assert_eq!(row(&f, &a).await, joined);
    let mut changed = serde_json::to_value(completion()).unwrap();
    changed["job"]["result"] = json!({"artifact": "different-result"});
    let changed = SharedPreparationCompletion::from_controller(changed).unwrap();
    assert!(matches!(
        f.bridge()
            .complete_shared_preparation(&b, &changed, &backend)
            .await,
        Err(SetupError::InvalidRecord)
    ));
    assert_eq!(row(&f, &a).await, joined);
    f.bridge().begin_shared_preparation_leave(&a).await.unwrap();
    assert!(!f.bridge().leave_shared_preparation(&a).await.unwrap());
    let a_left = row(&f, &a).await;
    assert_eq!(stored_member(&a_left, &a)["left"], true);
    assert_eq!(a_left.data.body["closed"], false);
    assert!(
        f.bridge()
            .shared_preparation_left_selection(&a)
            .await
            .unwrap()
            == (selected.clone(), false)
    );
    assert!(
        f.bridge()
            .shared_preparation_left_selection(&b)
            .await
            .is_err()
    );
    assert!(matches!(
        f.bridge().check_shared_preparation(&a, &backend).await,
        Err(SetupError::Denied)
    ));
    f.bridge()
        .check_shared_preparation(&b, &backend)
        .await
        .unwrap();
    f.bridge().begin_shared_preparation_leave(&b).await.unwrap();
    assert!(f.bridge().leave_shared_preparation(&b).await.unwrap());
    let closed = row(&f, &a).await;
    assert!(f.bridge().shared_preparation_selection(&a).await.unwrap() == selected);
    assert!(f.bridge().shared_preparation_selection(&b).await.unwrap() == selected);
    assert!(
        f.bridge()
            .shared_preparation_left_selection(&a)
            .await
            .unwrap()
            == (selected.clone(), true)
    );
    assert!(
        f.bridge()
            .shared_preparation_left_selection(&c)
            .await
            .is_err()
    );
    let events = f.store.read_events(a.key(), 0, 100).await.unwrap();
    assert_eq!(closed.data.body["closed"], true);
    assert_eq!(stored_member(&closed, &a), stored_member(&a_left, &a));
    for refused in [&a, &b, &c] {
        assert!(matches!(
            f.bridge()
                .join_shared_preparation(refused, &selected, &backend)
                .await,
            Err(SetupError::Denied)
        ));
    }
    assert!(matches!(
        f.bridge()
            .complete_shared_preparation(&b, &completion(), &backend)
            .await,
        Err(SetupError::Denied)
    ));
    assert!(f.bridge().leave_shared_preparation(&a).await.unwrap());
    assert_eq!(row(&f, &a).await, closed);
    assert_eq!(f.store.read_events(a.key(), 0, 100).await.unwrap(), events);
}

#[tokio::test]
async fn exact_account_context_and_actual_parent_edge_bound_sharing() {
    let mut f = Fixture::new();
    let (backend, parent) = backend(&f).await;
    let selected = selection("account-a", "context-a");
    let a = acquisition(&mut f, &backend, &parent, "child-a").await;
    let b = acquisition(&mut f, &backend, &parent, "child-b").await;
    let a = member(&f, &a, &selected);
    f.bridge()
        .join_shared_preparation(&a, &selected, &backend)
        .await
        .unwrap();
    let before = row(&f, &a).await;
    for changed in [
        selection("account-b", "context-a"),
        selection("account-a", "context-b"),
    ] {
        let other = member(&f, &b, &changed);
        assert_ne!(
            other.key(),
            a.key(),
            "same artifact is not exact request equality"
        );
        assert!(matches!(
            f.bridge()
                .join_shared_preparation(&a, &changed, &backend)
                .await,
            Err(SetupError::WrongScope)
        ));
        assert!(f.store.get(other.key()).await.unwrap().is_none());
        assert_eq!(row(&f, &a).await, before);
    }

    // A real, separately claimed parent with the same cycle label still has
    // no Parent edge to this child. Labels and owner-key shape cannot supply it.
    let other_parent = claimed(backend.service(), "other-parent").await;
    let old = b.request().host().work_context.as_ref().unwrap();
    let child = backend
        .service()
        .get(None, None, old.child().item_id.clone())
        .await
        .unwrap();
    let wrong = SetupWorkContext::from_owner(
        backend.pin().clone(),
        work_ref(&other_parent),
        old.child().clone(),
        other_parent.owner.as_ref().unwrap().key.clone(),
        child.owner.as_ref().unwrap().key.clone(),
        CYCLE.into(),
    )
    .unwrap();
    assert!(matches!(
        wrong.check_current(&backend).await,
        Err(SetupError::Denied)
    ));
    assert_eq!(row(&f, &a).await, before);
}

async fn terminal_evidence(
    f: &Fixture,
    snapshot: &SetupSnapshot,
    name: &str,
) -> ContinuationAdmissionEvidence {
    let lease = f.lease(snapshot).await;
    let ended = f
        .bridge()
        .resolve(
            snapshot,
            Resolution::Unavailable(reference(name)),
            Some(&lease),
            f.access(&f.context(220)),
        )
        .await
        .unwrap();
    let obligation = continuation_obligation(&ended.record, &ended.request).unwrap();
    let receipt = ContextReference {
        owner: id("runtime"),
        ..reference(name)
    };
    ContinuationOwner::new(0)
        .verify_admission(&obligation, &receipt)
        .await
        .unwrap()
        .unwrap()
}

// Same thin real-SQLite delegation pattern as work_executor::PausedEffectStore.
// Only the exact member-delivery CAS response is replaced, after SQLite has
// committed its record and event atomically. This models response loss; it is
// not a production store or a claim that the physical backend loses replies.
struct LostDeliveryReply<'a> {
    inner: &'a SqliteStore,
    key: StoreKey,
    armed: AtomicBool,
}

#[async_trait]
impl ToolkitStore for LostDeliveryReply<'_> {
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
        let selected = transition.key == self.key
            && transition.events.len() == 1
            && transition.events[0].kind == "member_delivery_observed";
        let committed = self.inner.compare_and_swap(transition).await?;
        if selected && self.armed.swap(false, Ordering::SeqCst) {
            return Err(StoreError::OwnerUnavailable);
        }
        Ok(committed)
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

#[tokio::test]
async fn failed_write_and_lost_committed_reply_replay_without_reopening_left_members() {
    let mut f = Fixture::new();
    let (backend, parent) = backend(&f).await;
    let selected = selection("account-a", "context-a");
    let a_snapshot = acquisition(&mut f, &backend, &parent, "child-a").await;
    let b_snapshot = acquisition(&mut f, &backend, &parent, "child-b").await;
    let a = member(&f, &a_snapshot, &selected);
    let b = member(&f, &b_snapshot, &selected);
    for current in [&a, &b] {
        f.bridge()
            .join_shared_preparation(current, &selected, &backend)
            .await
            .unwrap();
    }
    f.bridge()
        .complete_shared_preparation(&a, &completion(), &backend)
        .await
        .unwrap();
    let evidence_a = terminal_evidence(&f, &a_snapshot, "receipt-a").await;
    let evidence_b = terminal_evidence(&f, &b_snapshot, "receipt-b").await;
    let before = row(&f, &a).await;
    let events = f.store.read_events(a.key(), 0, 100).await.unwrap();
    let acquisition_a = f.store.get(a.acquisition()).await.unwrap();
    let acquisition_b = f.store.get(b.acquisition()).await.unwrap();
    let outbox_a = f
        .store
        .list_effects(a.acquisition(), None, 100)
        .await
        .unwrap();
    let outbox_b = f
        .store
        .list_effects(b.acquisition(), None, 100)
        .await
        .unwrap();
    assert!(!outbox_a.is_empty());
    assert!(!outbox_b.is_empty());
    let sql = rusqlite::Connection::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    sql.execute_batch(
        "CREATE TRIGGER fail_shared_delivery BEFORE UPDATE ON toolkit_records
        WHEN json_extract(NEW.data, '$.schema') = 'meerkat_toolkit.shared_preparation'
        BEGIN SELECT RAISE(ABORT, 'shared delivery write refused'); END;",
    )
    .unwrap();
    assert!(matches!(
        f.bridge()
            .acknowledge_shared_preparation_delivery(&a, &evidence_a)
            .await,
        Err(SetupError::Store(StoreError::Storage(_)))
    ));
    assert_eq!(row(&f, &a).await, before);
    assert_eq!(f.store.read_events(a.key(), 0, 100).await.unwrap(), events);
    assert_eq!(f.store.get(a.acquisition()).await.unwrap(), acquisition_a);
    assert_eq!(f.store.get(b.acquisition()).await.unwrap(), acquisition_b);
    assert_eq!(
        f.store
            .list_effects(a.acquisition(), None, 100)
            .await
            .unwrap(),
        outbox_a
    );
    assert_eq!(
        f.store
            .list_effects(b.acquisition(), None, 100)
            .await
            .unwrap(),
        outbox_b
    );
    sql.execute_batch("DROP TRIGGER fail_shared_delivery;")
        .unwrap();
    drop(sql);

    let lost = LostDeliveryReply {
        inner: &f.store,
        key: a.key().clone(),
        armed: AtomicBool::new(true),
    };
    let bridge = SetupStore::new(
        &lost,
        SetupScope::new(f.tenant.clone(), f.deployment.clone()),
    )
    .unwrap();
    assert!(matches!(
        bridge
            .acknowledge_shared_preparation_delivery(&a, &evidence_a)
            .await,
        Err(SetupError::Store(StoreError::OwnerUnavailable))
    ));
    assert!(!lost.armed.load(Ordering::SeqCst));
    let delivered = row(&f, &a).await;
    assert_eq!(delivered.revision, before.revision + 1);
    let committed_events = f.store.read_events(a.key(), 0, 100).await.unwrap();
    assert_eq!(committed_events.len(), events.len() + 1);
    assert_eq!(committed_events[..events.len()], events);
    let committed = committed_events.last().unwrap();
    assert_eq!(committed.kind, "member_delivery_observed");
    assert_eq!(committed.record_revision, delivered.revision);
    assert_eq!(committed.data, delivered.data);
    assert_eq!(
        stored_member(&delivered, &a)["delivered"]["evidence"],
        serde_json::to_value(&evidence_a).unwrap()
    );
    assert!(stored_member(&delivered, &b)["delivered"].is_null());
    assert_eq!(
        delivered.data.body["completion"],
        before.data.body["completion"]
    );
    // A fresh store owner sees the actual committed row after the synthetic
    // lost reply. Exact replay neither adds an event nor acknowledges outboxes.
    let reopened = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    f.for_store(&reopened)
        .acknowledge_shared_preparation_delivery(&a, &evidence_a)
        .await
        .unwrap();
    assert_eq!(row(&f, &a).await, delivered);
    assert_eq!(
        f.store.read_events(a.key(), 0, 100).await.unwrap(),
        committed_events
    );
    assert_eq!(f.store.get(a.acquisition()).await.unwrap(), acquisition_a);
    assert_eq!(f.store.get(b.acquisition()).await.unwrap(), acquisition_b);
    assert_eq!(
        f.store
            .list_effects(a.acquisition(), None, 100)
            .await
            .unwrap(),
        outbox_a
    );
    assert_eq!(
        f.store
            .list_effects(b.acquisition(), None, 100)
            .await
            .unwrap(),
        outbox_b
    );
    f.bridge().begin_shared_preparation_leave(&a).await.unwrap();
    assert!(!f.bridge().leave_shared_preparation(&a).await.unwrap());
    f.bridge().begin_shared_preparation_leave(&b).await.unwrap();
    assert!(f.bridge().leave_shared_preparation(&b).await.unwrap());
    let closed = row(&f, &a).await;
    let mut later = evidence_a.clone();
    later.observed_at_ms += 1;
    f.for_store(&reopened)
        .acknowledge_shared_preparation_delivery(&a, &later)
        .await
        .unwrap();
    assert_eq!(
        row(&f, &a).await,
        closed,
        "a later observation retains the first exact receipt"
    );

    // Historical native evidence can arrive after member leave. It cannot
    // clear either leave bit, replace another member, or reopen this group.
    f.for_store(&reopened)
        .acknowledge_shared_preparation_delivery(&b, &evidence_b)
        .await
        .unwrap();
    let historical = row(&f, &a).await;
    assert_eq!(historical.data.body["closed"], true);
    assert_eq!(stored_member(&historical, &a), stored_member(&closed, &a));
    let mut expected_b = stored_member(&closed, &b).clone();
    expected_b["delivered"] = json!({"evidence": evidence_b});
    assert_eq!(stored_member(&historical, &b), &expected_b);
    assert_eq!(
        historical.data.body["completion"],
        before.data.body["completion"]
    );
    let events = f.store.read_events(a.key(), 0, 100).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "member_delivery_observed")
            .count(),
        2
    );
    for field in [
        "receipt",
        "obligation",
        "delivery",
        "runtime",
        "session",
        "input",
        "clock",
    ] {
        let mut changed = evidence_a.clone();
        match field {
            "receipt" => {
                changed.receipt = ContextReference {
                    owner: id("runtime"),
                    ..reference("other-receipt")
                }
            }
            "obligation" => {
                changed.obligation_digest = ContentDigest::of_bytes(b"other obligation")
            }
            "delivery" => changed.delivery_identity = ContentDigest::of_bytes(b"other delivery"),
            "runtime" => changed.runtime = id("other-runtime"),
            "session" => changed.session = id("other-session"),
            "input" => changed.durable_input = reference("other-input"),
            "clock" => changed.observed_at_ms -= 1,
            _ => unreachable!(),
        }
        assert!(
            f.for_store(&reopened)
                .acknowledge_shared_preparation_delivery(&a, &changed)
                .await
                .is_err(),
            "{field}"
        );
        assert_eq!(row(&f, &a).await, historical, "{field}");
    }
    f.for_store(&reopened)
        .acknowledge_shared_preparation_delivery(&b, &evidence_b)
        .await
        .unwrap();
    assert_eq!(row(&f, &a).await, historical);
    assert_eq!(f.store.read_events(a.key(), 0, 100).await.unwrap(), events);
    assert_eq!(f.store.get(a.acquisition()).await.unwrap(), acquisition_a);
    assert_eq!(f.store.get(b.acquisition()).await.unwrap(), acquisition_b);
    assert_eq!(
        f.store
            .list_effects(a.acquisition(), None, 100)
            .await
            .unwrap(),
        outbox_a
    );
    assert_eq!(
        f.store
            .list_effects(b.acquisition(), None, 100)
            .await
            .unwrap(),
        outbox_b
    );
}

/// Complement to the live two-SDK-child case: this closes the actual writable
/// SQLite owner while both members are pending. Controller completion and
/// continuation admission remain mechanical owner inputs in this store test;
/// no process restart, native grant restoration or physical worker is claimed.
#[tokio::test]
async fn reopened_sqlite_owner_keeps_cancelled_a_separate_from_completed_b() {
    // The existing generic continuation fixture has one fixed input identity.
    // This local owner retains distinct exact admission evidence for A and B.
    struct ExactContinuation {
        expected: Value,
        evidence: ContinuationAdmissionEvidence,
    }
    #[async_trait]
    impl SetupContinuationOwner for ExactContinuation {
        async fn verify_admission(
            &self,
            obligation: &ContinuationObligation,
            receipt: &ContextReference,
        ) -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError> {
            assert_eq!(serde_json::to_value(obligation).unwrap(), self.expected);
            assert!(receipt == &self.evidence.receipt);
            Ok(Some(self.evidence.clone()))
        }
    }
    fn continuation(snapshot: &SetupSnapshot, name: &str) -> ExactContinuation {
        let obligation = continuation_obligation(&snapshot.record, &snapshot.request).unwrap();
        ExactContinuation {
            expected: serde_json::to_value(&obligation).unwrap(),
            evidence: ContinuationAdmissionEvidence {
                receipt: ContextReference {
                    owner: id("runtime"),
                    ..reference(&format!("receipt-{name}"))
                },
                obligation_digest: obligation.digest().unwrap(),
                delivery_identity: obligation.delivery_identity().unwrap(),
                runtime: id("runtime-a"),
                session: id(&format!("session-{name}")),
                durable_input: reference(&format!("durable-input-{name}")),
                observed_at_ms: obligation.terminal_at_ms() + 1,
            },
        }
    }

    let mut f = Fixture::new();
    let (backend, parent) = backend(&f).await;
    let selected = selection("account-a", "context-a");
    let a_snapshot = acquisition(&mut f, &backend, &parent, "child-a").await;
    let authority_a = f.authority.clone();
    let authorization_a = f.authorization.clone();

    // Same requester and target can own two independent exact work inputs.
    // Reconstruct B through the existing native test authority, not A's grant.
    f.authority = SetupRequestEvidence::new(
        native_fixture::association(
            principal("person-a"),
            f.destination.clone(),
            "original-work-b",
            "private-a",
        ),
        authority_a.constraints().clone(),
    )
    .unwrap();
    f.authorization = native_fixture::current(f.authority.association());
    let mut host_b = f.request.host().clone();
    host_b.parent_operation = Some(id("original-task-b"));
    host_b.continuation.task = id("original-task-b");
    f.request = validate_request(
        f.request.request().clone(),
        host_b,
        &f.authority,
        &f.context(200),
        &f.references,
    )
    .unwrap();
    let b_snapshot = acquisition(&mut f, &backend, &parent, "child-b").await;
    let a = member(&f, &a_snapshot, &selected);
    let b = member(&f, &b_snapshot, &selected);
    assert_eq!(a.key(), b.key());
    assert_ne!(a.acquisition(), b.acquisition());
    assert_ne!(a.source(), b.source());
    assert_ne!(authority_a.original_work(), f.authority.original_work());
    for current in [&a, &b] {
        f.bridge()
            .join_shared_preparation(current, &selected, &backend)
            .await
            .unwrap();
    }
    let pending = row(&f, &a).await;
    let pending_events = f.store.read_events(a.key(), 0, 100).await.unwrap();
    let a_before = f.store.get(a.acquisition()).await.unwrap().unwrap();
    let b_before = f.store.get(b.acquisition()).await.unwrap().unwrap();
    assert_eq!(pending.data.body["members"].as_object().unwrap().len(), 2);
    assert!(pending.data.body["completion"].is_null());
    assert_eq!(pending.data.body["closed"], false);

    // All operations above have returned. No SetupStore borrow, clone or
    // entered blocking operation retains the old SqliteStore connection.
    let database = f.directory.path().join("toolkit.sqlite");
    drop(f.store);
    f.store = SqliteStore::open_existing(&database).unwrap();
    assert_eq!(row(&f, &a).await, pending);
    assert_eq!(
        f.store.read_events(a.key(), 0, 100).await.unwrap(),
        pending_events
    );
    assert_eq!(
        f.store.get(a.acquisition()).await.unwrap().unwrap(),
        a_before
    );
    assert_eq!(
        f.store.get(b.acquisition()).await.unwrap().unwrap(),
        b_before
    );

    // Reopening a locator cannot borrow the other waiter's current authority.
    assert!(matches!(
        f.bridge()
            .cancel(
                &id(&a.acquisition().id),
                reference("wrong-cancel"),
                f.access(&f.context(220)),
            )
            .await,
        Err(SetupError::Request(RequestError::ChangedAuthority))
    ));
    assert_eq!(
        f.store.get(a.acquisition()).await.unwrap().unwrap(),
        a_before
    );
    assert_eq!(
        f.store.get(b.acquisition()).await.unwrap().unwrap(),
        b_before
    );
    assert_eq!(row(&f, &a).await, pending);
    let context_a = SetupUseContext {
        authorization: &authorization_a,
        ..f.context(220)
    };
    let cancelled = f
        .bridge()
        .cancel(
            &id(&a.acquisition().id),
            reference("cancelled-a"),
            SetupAccess {
                evidence: &authority_a,
                context: &context_a,
                references: &f.references,
            },
        )
        .await
        .unwrap();
    assert_eq!(cancelled.phase, AcquisitionPhase::Cancelled);
    assert_eq!(
        f.store.get(b.acquisition()).await.unwrap().unwrap(),
        b_before
    );
    f.bridge().begin_shared_preparation_leave(&a).await.unwrap();
    assert!(matches!(
        f.bridge().check_shared_preparation(&a, &backend).await,
        Err(SetupError::Denied)
    ));
    assert!(matches!(
        f.bridge()
            .complete_shared_preparation(&a, &completion(), &backend)
            .await,
        Err(SetupError::Denied)
    ));
    f.bridge()
        .check_shared_preparation(&b, &backend)
        .await
        .unwrap();
    f.bridge()
        .complete_shared_preparation(&b, &completion(), &backend)
        .await
        .unwrap();
    let pending_leave_completed = row(&f, &a).await;
    assert_eq!(
        pending_leave_completed.data.body["completion"],
        serde_json::to_value(completion()).unwrap()
    );
    assert!(!f.bridge().leave_shared_preparation(&a).await.unwrap());
    let left = row(&f, &a).await;
    assert_eq!(stored_member(&left, &a)["left"], true);
    assert_eq!(stored_member(&left, &b)["left"], false);
    assert_eq!(left.data.body["closed"], false);
    assert_eq!(
        left.data.body["completion"],
        pending_leave_completed.data.body["completion"]
    );
    assert!(matches!(
        f.bridge().check_shared_preparation(&a, &backend).await,
        Err(SetupError::Denied)
    ));
    f.bridge()
        .check_shared_preparation(&b, &backend)
        .await
        .unwrap();
    assert!(matches!(
        f.bridge()
            .complete_shared_preparation(&a, &completion(), &backend)
            .await,
        Err(SetupError::Denied)
    ));
    assert_eq!(row(&f, &a).await, left);

    f.bridge()
        .complete_shared_preparation(&b, &completion(), &backend)
        .await
        .unwrap();
    let completed = row(&f, &a).await;
    assert_eq!(
        completed, left,
        "B completion replay after durable A leave is unchanged"
    );
    assert_eq!(
        completed.data.body["completion"],
        serde_json::to_value(completion()).unwrap()
    );
    assert_eq!(stored_member(&completed, &a), stored_member(&left, &a));
    assert!(stored_member(&completed, &b)["delivered"].is_null());
    let lease_b = f.lease(&b_snapshot).await;
    let ended_b = f
        .bridge()
        .resolve(
            &b_snapshot,
            Resolution::Unavailable(reference("prepared-without-service")),
            Some(&lease_b),
            f.access(&f.context(230)),
        )
        .await
        .unwrap();
    let ended_a = f
        .bridge()
        .recover(f.store.get(a.acquisition()).await.unwrap().unwrap())
        .unwrap();
    let owner_a = continuation(&ended_a, "a");
    let owner_b = continuation(&ended_b, "b");
    assert_ne!(
        owner_a.evidence.obligation_digest,
        owner_b.evidence.obligation_digest
    );
    assert_ne!(
        owner_a.evidence.delivery_identity,
        owner_b.evidence.delivery_identity
    );
    assert!(owner_a.evidence.durable_input != owner_b.evidence.durable_input);
    assert_ne!(owner_a.evidence.session, owner_b.evidence.session);
    assert!(matches!(
        f.bridge()
            .acknowledge_shared_preparation_delivery(&b, &owner_a.evidence)
            .await,
        Err(SetupError::Continuation(ContinuationError::InvalidReceipt))
    ));
    assert_eq!(row(&f, &a).await, completed);

    // A's cancellation continuation is separate from B's completed preparation.
    // Only B receives the shared physical-completion delivery observation.
    let a_ack = f
        .bridge()
        .acknowledge_continuation(
            &id(&a.acquisition().id),
            &owner_a.evidence.receipt,
            &owner_a,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a_ack.phase, AcquisitionPhase::Cancelled);
    let a_after = f.store.get(a.acquisition()).await.unwrap().unwrap();
    let a_events = f.store.read_events(a.acquisition(), 0, 100).await.unwrap();
    let b_ack = f
        .bridge()
        .acknowledge_continuation(
            &id(&b.acquisition().id),
            &owner_b.evidence.receipt,
            &owner_b,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b_ack.phase, AcquisitionPhase::Unavailable);
    f.bridge()
        .acknowledge_shared_preparation_delivery(&b, &owner_b.evidence)
        .await
        .unwrap();
    let delivered = row(&f, &a).await;
    // B's exact receipt cannot stand in for A's independent cancellation.
    // A genuine A receipt remains admissible as history after leave by design.
    assert!(matches!(
        f.bridge()
            .acknowledge_shared_preparation_delivery(&a, &owner_b.evidence)
            .await,
        Err(SetupError::InvalidRecord)
    ));
    assert_eq!(row(&f, &a).await, delivered);
    assert!(stored_member(&delivered, &a)["delivered"].is_null());
    assert_eq!(
        stored_member(&delivered, &b)["delivered"]["evidence"],
        serde_json::to_value(&owner_b.evidence).unwrap()
    );
    assert_eq!(
        f.store.get(a.acquisition()).await.unwrap().unwrap(),
        a_after
    );
    assert_eq!(
        f.store.read_events(a.acquisition(), 0, 100).await.unwrap(),
        a_events
    );
    let delivery_events = f.store.read_events(a.key(), 0, 100).await.unwrap();
    let b_after = f.store.get(b.acquisition()).await.unwrap().unwrap();
    let b_events = f.store.read_events(b.acquisition(), 0, 100).await.unwrap();

    drop(f.store);
    f.store = SqliteStore::open_existing(&database).unwrap();
    let replay = f
        .bridge()
        .acknowledge_continuation(
            &id(&b.acquisition().id),
            &owner_b.evidence.receipt,
            &owner_b,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.revision, b_ack.revision);
    f.bridge()
        .acknowledge_shared_preparation_delivery(&b, &owner_b.evidence)
        .await
        .unwrap();
    assert_eq!(row(&f, &a).await, delivered);
    assert_eq!(
        f.store.read_events(a.key(), 0, 100).await.unwrap(),
        delivery_events
    );
    assert_eq!(
        f.store.get(a.acquisition()).await.unwrap().unwrap(),
        a_after
    );
    assert_eq!(
        f.store.get(b.acquisition()).await.unwrap().unwrap(),
        b_after
    );
    assert_eq!(
        f.store.read_events(b.acquisition(), 0, 100).await.unwrap(),
        b_events
    );
    assert!(matches!(
        f.bridge()
            .join_shared_preparation(&a, &selected, &backend)
            .await,
        Err(SetupError::Denied)
    ));
    assert_eq!(row(&f, &a).await, delivered);
    f.bridge().begin_shared_preparation_leave(&b).await.unwrap();
    assert!(f.bridge().leave_shared_preparation(&b).await.unwrap());
    assert_eq!(row(&f, &a).await.data.body["closed"], true);
}

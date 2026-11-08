//! Real admitted source and SQLite/WorkGraph ownership. The resolver is a
//! mechanical joined-result port, not physical installer or cold-G proof.
use super::*;
use futures::FutureExt;
use meerkat_workgraph::{
    ClaimWorkItemRequest, CreateWorkItemRequest, LinkWorkItemsRequest, SqliteWorkGraphStore,
    WorkEdgeKind, WorkGraphService, WorkItem, WorkItemRef, WorkNamespace, WorkOwner, WorkOwnerKey,
};
use std::{
    panic::AssertUnwindSafe,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

const CYCLE: &str = "settled-preparation-control";
fn work_ref(item: &WorkItem) -> WorkItemRef {
    WorkItemRef {
        realm_id: item.realm_id.clone(),
        namespace: item.namespace.clone(),
        item_id: item.id.clone(),
    }
}
async fn claim(service: &WorkGraphService, name: &str) -> WorkItem {
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

struct JoinedResource {
    completion: SharedPreparationCompletion,
    leaves: AtomicUsize,
    fail_leave: AtomicBool,
}
#[async_trait::async_trait]
impl SharedPreparationResource for JoinedResource {
    async fn completion(
        &self,
        _: &str,
        _: &SharedPreparationSelection,
    ) -> Result<Option<SharedPreparationCompletion>, PrivateOriginalError> {
        Ok(Some(self.completion.clone()))
    }
    async fn retain_leave(
        &self,
        _: SharedPreparationLeave,
    ) -> Result<serde_json::Value, PrivateOriginalError> {
        panic!("recovery must not recreate native setup_stop")
    }
    async fn leave(
        &self,
        _: &str,
        _: &SharedPreparationSelection,
        last: bool,
    ) -> Result<serde_json::Value, PrivateOriginalError> {
        self.leaves.fetch_add(1, Ordering::SeqCst);
        if self.fail_leave.swap(false, Ordering::SeqCst) {
            return Err(PrivateOriginalError::Unavailable);
        }
        Ok(serde_json::json!({"last":last,"worker_joined":true}))
    }
}
struct Resolver {
    resource: Arc<JoinedResource>,
    calls: AtomicUsize,
    pause_next: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    unavailable: AtomicBool,
}
#[async_trait::async_trait]
impl SettledSharedPreparationResolver for Resolver {
    async fn resolve(
        &self,
        _: &SharedPreparationMember,
        _: &SharedPreparationSelection,
    ) -> Result<Arc<dyn SharedPreparationResource>, PrivateOriginalError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.pause_next.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(PrivateOriginalError::Unavailable);
        }
        Ok(self.resource.clone())
    }
}

async fn exercise(cancel: bool) {
    let f = ContinuationFixture::for_continuation().await;
    let resolver = Arc::new(Resolver {
        resource: Arc::new(JoinedResource {
            completion: SharedPreparationCompletion::from_controller(serde_json::json!({
            "controller":"a".repeat(64), "validation_id":"b".repeat(32), "job":{
                "id":"c".repeat(32),"kind":"install","family":"selected-python",
                "status":"succeeded","result":{},"error":null}}))
            .unwrap(),
            leaves: AtomicUsize::new(0),
            fail_leave: AtomicBool::new(cancel),
        }),
        calls: AtomicUsize::new(0),
        pause_next: AtomicBool::new(cancel),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        unavailable: AtomicBool::new(false),
    });
    let result = AssertUnwindSafe(async {
        let submitted = f.submit().await;
        f.finish(&submitted).await.unwrap();
        let association = f.binding().await.unwrap().unwrap();
        let source = &f.source;
        let reference = source.retained.lock().unwrap().iter()
            .find(|held| held.original.association() == &association).unwrap().reference.clone();
        let service = WorkGraphService::with_scope(Arc::new(SqliteWorkGraphStore::open(
            f.source_path.with_extension("settled-work.sqlite")).unwrap()), f.scope.realm.clone(),
            WorkNamespace::new("settled-preparation").unwrap());
        let parent = claim(&service, "parent").await;
        let child = claim(&service, "child").await;
        service.link(LinkWorkItemsRequest { realm_id: None, namespace: None, kind: WorkEdgeKind::Parent,
            from_id: child.id.clone(), to_id: parent.id.clone() }).await.unwrap();
        let backend = SetupWorkBackend::new(service, "settled-sqlite".into(), Id::new("generation-one").unwrap(),
            &[meerkat_core::DurabilityDeclaration::durable("workgraph", meerkat_core::DurabilityResolution::Persistent)]).unwrap();
        let work = meerkat_toolkit::setup::SetupWorkContext::from_owner(backend.pin().clone(), work_ref(&parent),
            work_ref(&child), parent.owner.as_ref().unwrap().key.clone(), child.owner.as_ref().unwrap().key.clone(), CYCLE.into()).unwrap();
        let acquisition = super::super::admission_tests::shared_acquisition(&f, &f.probe.as_ref().unwrap().captured(), work).await;
        let selection = SharedPreparationSelection::from_controller(serde_json::json!({
            "controller":"a".repeat(64),"arguments_sha256":"d".repeat(64),"arguments":{},
            "effect":{"request":{"profile":"python"}},"plan":{},"result":{"validation_id":"b".repeat(32)}})).unwrap();
        let key = source.key(&reference).unwrap();
        let setup = source.service_setup_store().unwrap();
        let member = setup.shared_preparation_member(&acquisition, key.clone(), &selection).unwrap();
        setup.join_shared_preparation(&member, &selection, &backend).await.unwrap();
        // The source fixture retains the exact canonical membership before any
        // attachment. It supplies no new authority or physical completion.
        let row = source.store.get(&key).await.unwrap().unwrap();
        let mut record = source.decode_record(&row, &reference).unwrap();
        record.shared_preparations.push(member.clone());
        let row = source.store.compare_and_swap(toolkit_store::Transition { key: key.clone(),
            expected_revision: row.revision, expected_digest: Some(row.content_digest), claim: None,
            data: encode(record).unwrap(), events: vec![], effects: vec![] }).await.unwrap();
        source.bind_settled_shared_preparations(resolver.clone()).unwrap();
        let inputs = f.inputs().await.unwrap();
        if cancel {
            let resolving = source.shared_resource(&member);
            tokio::pin!(resolving);
            tokio::select! {
                _ = resolver.entered.notified() => (),
                _ = &mut resolving => panic!("resolver must pause before attachment"),
            }
            assert!(source.record_task_group_cancelled(&reference).await.is_err(), "lost physical acknowledgement remains unresolved");
            let left = source.store.get(member.key()).await.unwrap().unwrap();
            assert_eq!(left.data.body["closed"], true);
            resolver.release.notify_one();
            assert!(matches!(resolving.await, Err(PrivateOriginalError::Conflict)), "cancellation changed the source during resolution");
            source.record_task_group_cancelled(&reference).await.unwrap();
            assert_eq!(source.store.get(member.key()).await.unwrap().unwrap(), left, "leave replay does not rewrite P");
            assert_eq!(resolver.resource.leaves.load(Ordering::SeqCst), 2);
            assert_eq!(resolver.calls.load(Ordering::SeqCst), 2, "cleanup reuses its retained attachment");
            let after = source.store.get(&key).await.unwrap().unwrap();
            assert!(source.decode_record(&after, &reference).unwrap().operation_cancelled);
            source.recover_settled_shared_preparations(&after, &backend).await.unwrap();
            assert!(source.store.get(member.key()).await.unwrap().unwrap().data.body["completion"].is_null());
        } else {
            let pending = source.store.get(member.key()).await.unwrap().unwrap();
            let events = source.store.read_events(member.key(), 0, 100).await.unwrap();
            // Losing this metadata observation cannot repeat physical work or
            // publish completion. The joined resource remains its only source.
            resolver.pause_next.store(true, Ordering::SeqCst);
            {
                let recovering = source.recover_settled_shared_preparations(&row, &backend);
                tokio::pin!(recovering);
                tokio::select! {
                    _ = resolver.entered.notified() => (),
                    _ = &mut recovering => panic!("completion resolution must pause"),
                }
            }
            assert_eq!(source.store.get(member.key()).await.unwrap().unwrap(), pending);
            // Fail only P completion publication in the actual SQLite owner.
            // Neither the canonical row nor its event is acknowledged on loss.
            let sql = rusqlite::Connection::open(&f.source_path).unwrap();
            sql.execute_batch("CREATE TRIGGER fail_p_completion BEFORE UPDATE OF data ON toolkit_records
                WHEN json_extract(NEW.data, '$.schema') = 'meerkat_toolkit.shared_preparation'
                  AND json_extract(OLD.data, '$.body.completion') IS NULL
                  AND json_extract(NEW.data, '$.body.completion') IS NOT NULL
                BEGIN SELECT RAISE(ABORT, 'P completion write refused'); END;").unwrap();
            assert!(matches!(source.recover_settled_shared_preparations(&row, &backend).await,
                Err(PrivateOriginalError::Unavailable)));
            assert_eq!(source.store.get(member.key()).await.unwrap().unwrap(), pending);
            assert_eq!(source.store.read_events(member.key(), 0, 100).await.unwrap(), events);
            assert_eq!(source.store.get(&row.key).await.unwrap().unwrap(), row);
            assert_eq!(f.inputs().await.unwrap(), inputs);
            sql.execute_batch("DROP TRIGGER fail_p_completion;").unwrap();
            source.recover_settled_shared_preparations(&row, &backend).await.unwrap();
            let completed = source.store.get(member.key()).await.unwrap().unwrap();
            assert!(!completed.data.body["completion"].is_null());
            let first = source.shared_resource(&member).await.unwrap().0;
            source.recover_settled_shared_preparations(&row, &backend).await.unwrap();
            let second = source.shared_resource(&member).await.unwrap().0;
            assert!(Arc::ptr_eq(&first, &second));
            assert_eq!(resolver.calls.load(Ordering::SeqCst), 2,
                "dropped resolution never attaches; failed publication retains its exact attachment");
            assert_eq!(source.store.get(member.key()).await.unwrap().unwrap(), completed);
            // A fresh owner without an admitted original cannot attach using
            // only stored source/P rows, even when the resolver has evidence.
            source.retained.lock().unwrap().clear();
            assert!(matches!(source.shared_resource(&member).await, Err(PrivateOriginalError::Unavailable)));
            assert_eq!(resolver.calls.load(Ordering::SeqCst), 2);
        }
        assert_eq!(f.inputs().await.unwrap(), inputs, "attachment/leave admits no input");
    }).catch_unwind().await;
    resolver.release.notify_waiters();
    let cleanup = f.shutdown().await;
    assert!(cleanup.is_ok());
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settled_source_attachment_seals_once_and_requires_retained_original() {
    exercise(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_attachment_rechecks_source_and_replays_left_physical_ack() {
    exercise(true).await;
}

/// Build P from the real admitted fixture input and canonical WorkGraph parent.
/// Only the physical selected-check/completion port is mechanical in this test.
async fn settled_member<S>(
    f: &ContinuationFixture<S>,
    completion: &SharedPreparationCompletion,
) -> (PrivateOriginalRef, SharedPreparationMember) {
    let submitted = f.submit().await;
    f.finish(&submitted).await.unwrap();
    let reference = f.original_reference().clone();
    let service = WorkGraphService::with_scope(
        Arc::new(
            SqliteWorkGraphStore::open(f.source_path.with_extension("cold-settled-work.sqlite"))
                .unwrap(),
        ),
        f.scope.realm.clone(),
        WorkNamespace::new("cold-settled-preparation").unwrap(),
    );
    let parent = claim(&service, "parent").await;
    let child = claim(&service, "child").await;
    service
        .link(LinkWorkItemsRequest {
            realm_id: None,
            namespace: None,
            kind: WorkEdgeKind::Parent,
            from_id: child.id.clone(),
            to_id: parent.id.clone(),
        })
        .await
        .unwrap();
    let backend = SetupWorkBackend::new(
        service,
        "settled-sqlite".into(),
        Id::new("generation-one").unwrap(),
        &[meerkat_core::DurabilityDeclaration::durable(
            "workgraph",
            meerkat_core::DurabilityResolution::Persistent,
        )],
    )
    .unwrap();
    let work = meerkat_toolkit::setup::SetupWorkContext::from_owner(
        backend.pin().clone(),
        work_ref(&parent),
        work_ref(&child),
        parent.owner.as_ref().unwrap().key.clone(),
        child.owner.as_ref().unwrap().key.clone(),
        CYCLE.into(),
    )
    .unwrap();
    let acquisition = super::super::admission_tests::shared_acquisition(
        f,
        &f.probe.as_ref().unwrap().captured(),
        work,
    )
    .await;
    let selection = SharedPreparationSelection::from_controller(serde_json::json!({
        "controller":"a".repeat(64),"arguments_sha256":"d".repeat(64),"arguments":{},
        "effect":{"request":{"profile":"python"}},"plan":{},"result":{"validation_id":"b".repeat(32)}})).unwrap();
    let key = f.source.key(&reference).unwrap();
    let setup = f.source.service_setup_store().unwrap();
    let member = setup
        .shared_preparation_member(&acquisition, key.clone(), &selection)
        .unwrap();
    setup
        .join_shared_preparation(&member, &selection, &backend)
        .await
        .unwrap();
    setup
        .complete_shared_preparation(&member, completion, &backend)
        .await
        .unwrap();
    let row = f.store.get(&key).await.unwrap().unwrap();
    let mut record = f.source.decode_record(&row, &reference).unwrap();
    record.shared_preparations.push(member.clone());
    f.store
        .compare_and_swap(toolkit_store::Transition {
            key,
            expected_revision: row.revision,
            expected_digest: Some(row.content_digest),
            claim: None,
            data: encode(record).unwrap(),
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap();
    (reference, member)
}

async fn replace_data(
    source: &PrivateOriginals,
    key: &toolkit_store::StoreKey,
    data: toolkit_store::RecordData,
) {
    let row = source.store.get(key).await.unwrap().unwrap();
    source
        .store
        .compare_and_swap(toolkit_store::Transition {
            key: key.clone(),
            expected_revision: row.revision,
            expected_digest: Some(row.content_digest),
            claim: None,
            data,
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_cancelled_source_acknowledges_only_settled_left_resource_without_reopening_original()
{
    cold_cancelled_cleanup(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_cancelled_source_reconciliation_retries_unavailable_settled_cleanup() {
    cold_cancelled_cleanup(true).await;
}

async fn cancel_or_reconcile(
    source: &PrivateOriginals,
    reference: &PrivateOriginalRef,
    automatic: bool,
) -> Result<PrivateOriginalCancellation, PrivateOriginalError> {
    if automatic {
        let row = source
            .store
            .get(&source.key(reference)?)
            .await
            .unwrap()
            .unwrap();
        source.reconcile_cancelled_workflow_source(&row).await
    } else {
        source.cancel_original_operation(reference).await
    }
}

async fn cold_cancelled_cleanup(automatic: bool) {
    let completion = SharedPreparationCompletion::from_controller(serde_json::json!({
        "controller":"a".repeat(64), "validation_id":"b".repeat(32), "job":{
            "id":"c".repeat(32),"kind":"install","family":"selected-python",
            "status":"succeeded","result":{},"error":null}}))
    .unwrap();
    let (first, history, retained, continuation) =
        ContinuationFixture::for_durable_continuation().await;
    let prepared = AssertUnwindSafe(Box::pin(async {
        let (reference, member) = settled_member(&first, &completion).await;
        // The canonical source and P leave settle before a missing physical
        // acknowledgement. No resource is synthesized to make this call pass.
        assert!(matches!(
            first.source.cancel_original_operation(&reference).await,
            Err(PrivateOriginalError::Unavailable)
        ));
        let source = first.store.get(member.source()).await.unwrap().unwrap();
        let p = first.store.get(member.key()).await.unwrap().unwrap();
        assert!(
            first
                .source
                .decode_record(&source, &reference)
                .unwrap()
                .operation_cancelled
        );
        assert!(
            first
                .source
                .service_setup_store()
                .unwrap()
                .shared_preparation_left_selection(&member)
                .await
                .unwrap()
                .1
        );
        (reference, member, source, p)
    }))
    .catch_unwind()
    .await;
    let closed = first.close_durable_owners().await;
    drop(continuation);
    drop(history);
    drop(first);
    assert!(
        closed.is_ok(),
        "old native, source and grant owners must join: {closed:?}"
    );
    let (reference, member, original_source, original_p) = match prepared {
        Ok(value) => value,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    let (f, history, continuation) =
        ContinuationFixture::reopen_durable_continuation(retained).await;
    let resolver = Arc::new(Resolver {
        resource: Arc::new(JoinedResource {
            completion,
            leaves: AtomicUsize::new(0),
            fail_leave: AtomicBool::new(true),
        }),
        calls: AtomicUsize::new(0),
        pause_next: AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        unavailable: AtomicBool::new(false),
    });
    let checked = AssertUnwindSafe(Box::pin(async {
        let source = &f.source;
        let association = source
            .decode_record(&original_source, &reference)
            .unwrap()
            .final_association
            .unwrap();
        assert!(
            source.retained.lock().unwrap().is_empty(),
            "actual new process owner has no original handles"
        );
        assert!(
            source
                .owner
                .historical_task_custody_for_test(&association)
                .is_none()
        );
        source
            .bind_settled_shared_preparations(resolver.clone())
            .unwrap();
        let before_inputs = f.inputs().await.unwrap();

        // The public command and existing recovery entry must both refuse drift
        // while their resource read is pending, even while cancellation is true.
        for key in [member.source(), member.key()] {
            let saved = source.store.get(key).await.unwrap().unwrap();
            resolver.pause_next.store(true, Ordering::SeqCst);
            let pending = cancel_or_reconcile(source, &reference, automatic);
            tokio::pin!(pending);
            tokio::select! {
                _ = resolver.entered.notified() => (),
                _ = &mut pending => panic!("closed-resource read must remain held"),
            }
            let mut changed = saved.data.clone();
            if key == member.source() {
                changed.body["shared_preparations"] = serde_json::json!([]);
            } else {
                changed.body["completion"]["job"]["result"] = serde_json::json!({"different":true});
            }
            replace_data(source, key, changed).await;
            resolver.release.notify_one();
            assert!(matches!(pending.await, Err(PrivateOriginalError::Conflict)));
            assert_eq!(resolver.resource.leaves.load(Ordering::SeqCst), 0);
            replace_data(source, key, saved.data).await;
        }
        let canonical_source = source.store.get(member.source()).await.unwrap().unwrap();
        let canonical_p = source.store.get(member.key()).await.unwrap().unwrap();
        let mut corrupt = canonical_p.data.clone();
        corrupt.body["members"] = serde_json::Value::Null;
        replace_data(source, member.key(), corrupt).await;
        let reads = resolver.calls.load(Ordering::SeqCst);
        assert!(
            cancel_or_reconcile(source, &reference, automatic)
                .await
                .is_err()
        );
        assert_eq!(
            resolver.calls.load(Ordering::SeqCst),
            reads,
            "corrupt P never reaches the physical owner"
        );
        replace_data(source, member.key(), canonical_p.data).await;
        resolver.unavailable.store(true, Ordering::SeqCst);
        // The recovery loop treats this same Unavailable result as a delayed
        // sweep. It must not reach workflow departure or report cleanup done.
        assert!(matches!(
            cancel_or_reconcile(source, &reference, automatic).await,
            Err(PrivateOriginalError::Unavailable)
        ));
        assert_eq!(
            resolver.resource.leaves.load(Ordering::SeqCst),
            0,
            "unsettled/missing physical evidence cannot acknowledge"
        );
        resolver.unavailable.store(false, Ordering::SeqCst);
        assert_eq!(
            f.inputs().await.unwrap(),
            before_inputs,
            "refused cleanup never resubmits A"
        );

        // Hold a genuinely separate input in the actual current actor while
        // A's lost cleanup acknowledgement is reconciled. Its model response
        // must still complete after A is acknowledged, without a retry.
        let mut event = crate::original_ingress::tests::private_event("after-cold-cancel-b");
        event.route = reference.route.clone();
        let content = "Read my latest messages";
        event.content_sha256 = digest(content.as_bytes());
        let authentication = event.original_authentication.clone();
        let b = source.retain_event(event).await.unwrap();
        let hold_b = f.hold_next_response();
        let admitted = source
            .submit_authenticated_original(PrivateOriginalSubmit {
                original: b,
                content: content.into(),
                transport_authentication: authentication,
            })
            .await
            .map_err(|error| error.to_string());
        assert!(
            admitted.is_ok(),
            "independent B must enter before old P cleanup"
        );
        tokio::time::timeout(std::time::Duration::from_secs(10), hold_b.entered())
            .await
            .unwrap();
        let with_b_inputs = f.inputs().await.unwrap();

        // Lost ACK remains repeatable through the real cold cancellation owner.
        // There is still no native-bound original or completion attachment.
        let final_p = source.store.get(member.key()).await.unwrap().unwrap();
        assert!(matches!(
            cancel_or_reconcile(source, &reference, automatic).await,
            Err(PrivateOriginalError::Unavailable)
        ));
        for _ in 0..2 {
            assert!(
                cancel_or_reconcile(source, &reference, automatic)
                    .await
                    .unwrap()
                    == PrivateOriginalCancellation::OperationsCancelled
            );
            assert_eq!(
                source.store.get(member.source()).await.unwrap().unwrap(),
                canonical_source
            );
            assert_eq!(
                source.store.get(member.key()).await.unwrap().unwrap(),
                final_p
            );
            assert!(
                !source
                    .retained
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|held| held.reference == reference)
            );
            assert!(
                source
                    .owner
                    .historical_task_custody_for_test(&association)
                    .is_none()
            );
        }
        assert_eq!(
            f.inputs().await.unwrap(),
            with_b_inputs,
            "cleanup never resubmits A or B"
        );
        assert!(
            source.shared_resource(&member).await.is_err(),
            "cancelled cleanup does not create a completion attachment"
        );
        assert_eq!(
            final_p.data, original_p.data,
            "old P completion and membership remain exact"
        );

        hold_b.release();
        f.finish(&admitted).await.unwrap();
        assert!(source.store.get(member.source()).await.unwrap().unwrap() == canonical_source);
    }))
    .catch_unwind()
    .await;
    resolver.release.notify_waiters();
    let closed = f.close_durable_owners().await;
    drop(continuation);
    drop(history);
    assert!(closed.is_ok(), "restored owners must join: {closed:?}");
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
}

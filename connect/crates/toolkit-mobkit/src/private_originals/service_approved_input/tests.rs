//! The original is actually admitted by the existing native machine fixture.
//! Linked B rows are canonical source fixtures with historical coordinates only;
//! they do not claim human approval, a usable grant or a submitted B input.
use super::*;
use futures::FutureExt;
use std::{panic::AssertUnwindSafe, sync::atomic::{AtomicBool, Ordering}};
use meerkat_authorization_contracts::{evidence::EvidenceDigest, work_association::InputAuthorityAssociation};

async fn cancellation_case(parent_only_commit: bool, historical_obstacles: bool, admitted_partial_write: Option<bool>) {
    let fixture = ContinuationFixture::for_continuation().await;
    let result = AssertUnwindSafe(async {
        let submitted = fixture.submit().await;
        fixture.finish(&submitted).await.unwrap();
        assert_eq!(fixture.inputs().await, Ok(1));
        let a = fixture.binding().await.unwrap().unwrap();
        let source = &fixture.source;
        let parent = source.retained.lock().unwrap().iter()
            .find(|held| held.original.association() == &a).unwrap().reference.clone();
        let parent_stored = source.store.get(&source.key(&parent).unwrap()).await.unwrap().unwrap();
        let parent_record = source.decode_record(&parent_stored, &parent).unwrap();
        let dispatch = fixture.probe.as_ref().unwrap().captured();
        // The cancellation sweep also projects the linked setup. Retain its
        // real canonical acquisition without reserving or authorizing B.
        let acquisition = admission_tests::acquisition(&fixture, &dispatch).await;
        assert!(acquisition.approved_service().is_none());
        let original_task = acquisition.request().host().continuation.native_original
            .as_ref().unwrap().clone();
        let event = PrivateAuthenticatedEvent {
            route: parent.route.clone(), original: EvidenceId::new("linked-b").unwrap(),
            occurrence: EvidenceId::new("linked-b").unwrap(),
            original_authentication: PrivateSourceEvidence {
                resource_id: EvidenceId::new("linked-b").unwrap(), revision: EvidenceId::new("1").unwrap(),
                sha256: digest(b"linked-b"),
            },
            root_event: PrivateSourceEvidence {
                resource_id: EvidenceId::new("linked-b").unwrap(), revision: EvidenceId::new("1").unwrap(),
                sha256: digest(b"linked-b"),
            },
            contributors: parent_record.event.contributors.clone(),
            content_sha256: digest(acquisition.request().request().request.as_bytes()),
        };
        let reference = PrivateOriginalRef { route: event.route.clone(), original: event.original.clone(),
            event_sha256: digest(&canonical_json(&event).unwrap()) };
        let key = source.key(&reference).unwrap();
        let link = ApprovedTaskLink::single(ApprovedTaskHeader {
            parent: parent.clone(), acquisition: acquisition.key().clone(), original_task, before_ms: u64::MAX },
            // Deliberately unresolved: cancellation may not require a current
            // grant, and these coordinates cannot admit or authorize B.
            PrivateServiceGrantRef { route: parent.route.clone(), decision: Id::new("unissued-b").unwrap(),
                decision_sha256: "0".repeat(64) });
        let record = SourceRecord { custody_attempt: None, bound_service_input: None, setup_request: None, configured_setup: None, service_setup_children: vec![], source: parent_record.source.clone(), event,
            final_association: None, operation_cancelled: false, service_attachment_failure: None, service_attachment_observation: None,
            service_attachment_member: None, approved_task: Some(link),
            // This exercises the actual canonical admitted-field branch. It
            // does not fabricate native B acceptance or a usable B grant.
            service_admission: admitted_partial_write.is_some().then(|| ServiceInputAdmission {
                session_id: fixture.session.to_string(), input_id: "historical-admitted-b".into(),
            }) };
        source.store.admit(AdmissionRequest { key: key.clone(), admission_key: key.id.clone(),
            data: encode(record.clone()).unwrap(), events: vec![], effects: vec![] }).await.unwrap();
        if historical_obstacles {
            for (name, completed) in [("historical-completed", true), ("historical-unavailable", false)] {
                let mut old = record.clone();
                old.event.route = Id::new("removed-application-route").unwrap();
                old.event.original = if completed { EvidenceId::new(name).unwrap() } else {
                    (0..1024).map(|index| EvidenceId::new(format!("{name}-{index}")).unwrap())
                        .find(|original| source_key_id(&old.source.realm, &old.source.ingress,
                            &old.source.source_authority, original).unwrap() < key.id)
                        .expect("unavailable fixture must sort before the current child")
                };
                old.event.occurrence = old.event.original.clone();
                let old_link = old.approved_task.as_mut().unwrap();
                old_link.header_mut().parent.route = old.event.route.clone();
                old_link.header_mut().parent.original = EvidenceId::new("unrelated-old-parent").unwrap();
                old.service_admission = completed.then(|| ServiceInputAdmission {
                    session_id: "historical-session".into(), input_id: "historical-input".into(),
                });
                let old_key = StoreKey { realm: source.scope.realm.clone(), owner: source.scope.owner.clone(),
                    id: source_key_id(&old.source.realm, &old.source.ingress, &old.source.source_authority,
                        &old.event.original).unwrap() };
                source.store.admit(AdmissionRequest { key: old_key.clone(), admission_key: old_key.id,
                    data: encode(old).unwrap(), events: vec![], effects: vec![] }).await.unwrap();
            }
        }
        let mut candidate = a.candidate().clone();
        candidate.original_work.work = reference.original.clone();
        candidate.ingress_namespace.occurrence_scope = reference.original.clone();
        for evidence in [&mut candidate.original_authentication, &mut candidate.root_event] {
            evidence.resource.resource_id = "linked-b".into();
            evidence.revision = EvidenceId::new("1").unwrap();
            evidence.digest = EvidenceDigest::of_bytes(b"linked-b");
        }
        let b = InputAuthorityAssociation::new(candidate).unwrap();
        let mut result_ancestors = Vec::new();
        source.extend_result_ancestor_sources(&record, &mut result_ancestors).await.unwrap();
        assert_eq!(result_ancestors.len(), 1);
        assert_eq!(result_ancestors[0].key, parent_stored.key);
        assert_eq!(result_ancestors[0].revision, parent_stored.revision);
        assert_eq!(result_ancestors[0].content_digest, parent_stored.content_digest);
        source.extend_result_ancestor_sources(&record, &mut result_ancestors).await.unwrap();
        assert_eq!(result_ancestors.len(), 1, "identical ancestor reads deduplicate");
        let mut conflicting = result_ancestors.clone();
        conflicting[0].revision += 1;
        assert!(matches!(source.extend_result_ancestor_sources(&record, &mut conflicting).await,
            Err(PrivateOriginalError::Conflict)));
        if let Some(persist_child_before_error) = admitted_partial_write {
            let child_stored = source.store.get(&key).await.unwrap().unwrap();
            let affected = [parent_stored, child_stored].into_iter().map(|stored| {
                let record = source.decode_stored(&stored).unwrap();
                OriginalCancellationSource {
                    realm: record.source.realm,
                    original: meerkat_authorization_contracts::work_association::OriginalWorkRef {
                        authority: record.source.source_authority, work: record.event.original,
                    },
                    association: record.final_association,
                    record: Arc::new(stored),
                }
            }).collect::<Vec<_>>();
            let failed_child_write = AtomicBool::new(false);
            // Exercise the existing writer: it marks BOTH sources Pending,
            // persists the exact parent, then encounters the child write fault.
            let partial = source.owner.record_linked_operations_cancelled(
                || async { Ok(affected) },
                |stored| {
                    let key = &key;
                    let failed_child_write = &failed_child_write;
                    async move {
                        if &stored.key == key {
                            if persist_child_before_error {
                                source.persist_source_cancellation(stored).await?;
                            }
                            failed_child_write.store(true, Ordering::SeqCst);
                            Err(OriginalWriterError::Unavailable)
                        } else {
                            source.persist_source_cancellation(stored).await
                        }
                    }
                },
            ).await;
            assert!(matches!(partial, Err(OriginalWriterError::Unavailable)));
            assert!(failed_child_write.load(Ordering::SeqCst));
            let before_a = source.store.get(&source.key(&parent).unwrap()).await.unwrap().unwrap();
            let before_b = source.store.get(&key).await.unwrap().unwrap();
            assert!(source.decode_record(&before_a, &parent).unwrap().operation_cancelled);
            let before_b = source.decode_record(&before_b, &reference).unwrap();
            assert_eq!(before_b.operation_cancelled, persist_child_before_error);
            assert!(matches!(source.extend_result_ancestor_sources(&before_b, &mut Vec::new()).await,
                Err(PrivateOriginalError::Denied)), "durable parent cancellation fences result before child repair");
            assert!(before_b.service_admission.is_some());
            for association in [&a, &b] {
                assert!(matches!(source.owner.original_disposition(
                    &association.candidate().ingress_namespace,
                    &association.candidate().original_work,
                ), Err(meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable)));
            }
            let mut after = None;
            loop {
                let page = source.step_service_inputs(&fixture.store, after.as_deref(), 1).await.unwrap();
                assert!(!page.had_unavailable_records);
                after = page.next_after_id;
                if after.is_none() { break; }
            }
            let after_a = source.store.get(&source.key(&parent).unwrap()).await.unwrap().unwrap();
            let after_b_stored = source.store.get(&key).await.unwrap().unwrap();
            assert!(source.decode_record(&after_a, &parent).unwrap().operation_cancelled);
            let after_b = source.decode_record(&after_b_stored, &reference).unwrap();
            assert!(after_b.operation_cancelled);
            assert!(after_b.service_admission == record.service_admission);
            for association in [&a, &b] {
                assert!(matches!(source.owner.original_disposition(
                    &association.candidate().ingress_namespace,
                    &association.candidate().original_work,
                ), Ok(OriginalDisposition::OperationCancelled { .. })));
            }
            // Once both canonical records and both owner dispositions have
            // settled, another real service sweep must leave the same native
            // authorization publication observation current.
            let (_, stamp) = source.owner.publication().observe(|| ()).unwrap();
            let mut after = None;
            loop {
                let page = source.step_service_inputs(&fixture.store, after.as_deref(), 1).await.unwrap();
                assert!(!page.had_unavailable_records);
                after = page.next_after_id;
                if after.is_none() { break; }
            }
            stamp.check_current().unwrap();
            let unchanged_a = source.store.get(&source.key(&parent).unwrap()).await.unwrap().unwrap();
            let unchanged_b = source.store.get(&key).await.unwrap().unwrap();
            assert_eq!(unchanged_a, after_a);
            assert_eq!(unchanged_b, after_b_stored);
            for association in [&a, &b] {
                assert!(matches!(source.owner.original_disposition(
                    &association.candidate().ingress_namespace,
                    &association.candidate().original_work,
                ), Ok(OriginalDisposition::OperationCancelled { .. })));
            }
        } else if parent_only_commit {
            // Model the real crash boundary: A's exact cancellation is durable,
            // B has not been updated and no in-memory cancellation was marked.
            source.persist_source_cancellation(Arc::new(parent_stored)).await.unwrap();
            let b_before = source.store.get(&key).await.unwrap().unwrap();
            assert!(!source.decode_record(&b_before, &reference).unwrap().operation_cancelled);
            assert!(matches!(source.extend_result_ancestor_sources(&record, &mut Vec::new()).await,
                Err(PrivateOriginalError::Denied)), "unchanged child cannot conceal a cancelled parent");
            assert!(matches!(source.owner.original_disposition(
                &a.candidate().ingress_namespace, &a.candidate().original_work,
            ), Ok(OriginalDisposition::Active)));
            if historical_obstacles {
                // Exercise the actual paged worker method, with an unavailable
                // historical row before the linked B that needs cancellation.
                let mut after = None;
                let mut had_unavailable = false;
                loop {
                    let page = source.step_service_inputs(&fixture.store, after.as_deref(), 1).await.unwrap();
                    had_unavailable |= page.had_unavailable_records;
                    after = page.next_after_id;
                    if after.is_none() { break; }
                }
                assert!(had_unavailable);
                let current = source.store.get(&key).await.unwrap().unwrap();
                assert!(source.decode_record(&current, &reference).unwrap().operation_cancelled);
            }
        } else {
            source.record_task_group_cancelled(&parent).await.unwrap();
            let stored_a = source.store.get(&source.key(&parent).unwrap()).await.unwrap().unwrap();
            let stored_b = source.store.get(&key).await.unwrap().unwrap();
            assert!(source.decode_record(&stored_a, &parent).unwrap().operation_cancelled);
            assert!(source.decode_record(&stored_b, &reference).unwrap().operation_cancelled);
            let entered = AtomicBool::new(false);
            let late = source.owner.retain_followup_source(&a, || async {
                entered.store(true, Ordering::SeqCst); Ok(())
            }).await;
            assert!(matches!(late, Err(OriginalWriterError::OperationCancelled)));
            assert!(!entered.load(Ordering::SeqCst), "cancelled A refuses before B publication callback");
        }
        // B has no local retained handle. The actual binder rereads canonical
        // B and its cancelled parent A through this same source owner.
        assert!(!source.retained.lock().unwrap().iter()
            .any(|held| held.reference == reference));
        let outcome = source.persist_or_observe_binding(&reference, &b, true).await;
        assert!(matches!(outcome, Err(OriginalWriterError::OperationCancelled)));
        let stored_b = source.store.get(&key).await.unwrap().unwrap();
        let b_after = source.decode_record(&stored_b, &reference).unwrap();
        assert!(b_after.operation_cancelled);
        assert!(b_after.final_association.is_none());
        assert_eq!(fixture.inputs().await, Ok(1), "source cancellation never admits B");
    }).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_instruction_cancellation_fences_existing_and_future_linked_sources() {
    cancellation_case(false, false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_instruction_parent_only_commit_refuses_unbound_b_after_canonical_reread() {
    cancellation_case(true, false, None).await;
}


#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removed_unrelated_route_cannot_block_current_cancellation_group() {
    cancellation_case(false, true, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn historical_unavailable_row_does_not_starve_later_service_recovery() {
    cancellation_case(true, true, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admitted_child_repairs_both_pending_dispositions_after_parent_only_cancellation_write() {
    cancellation_case(false, false, Some(false)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_cancelled_group_still_repairs_pending_dispositions_before_becoming_idempotent() {
    cancellation_case(false, false, Some(true)).await;
}

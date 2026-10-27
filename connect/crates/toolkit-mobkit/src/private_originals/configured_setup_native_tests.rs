//! Actual configured-parent producer and native first-input controls. The model
//! is scripted; no HTTP Human, Forge transaction or tool acquisition is claimed.
use super::*;
use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
use std::panic::AssertUnwindSafe;

async fn configured() -> FirstFixture {
    Box::pin(FirstFixture::new_selected_configuration(
        true, false, true, None, None, true,
    ))
    .await
}
fn parent(fixture: &FirstFixture) -> GrantLineageRef {
    fixture
        .lifetime()
        .selected_policy()
        .native_host
        .as_ref()
        .unwrap()
        .members[0]
        .setup_capability
        .as_ref()
        .unwrap()
        .parent
        .clone()
}
async fn unchanged_replay(
    fixture: &FirstFixture,
    first: &Result<PrivateOriginalSubmission, String>,
) {
    let before = fixture.binding().await.unwrap().unwrap();
    let count = fixture.model_requests();
    let replay = fixture.submit().await;
    fixture.finish(&replay).await.unwrap();
    let (
        Ok(PrivateOriginalSubmission::Admitted {
            input_id: a,
            deduplicated: false,
            turn_ticket: Some(_),
            source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
            ..
        }),
        Ok(PrivateOriginalSubmission::Admitted {
            input_id: b,
            deduplicated: true,
            turn_ticket: Some(_),
            source_settlement: PrivateSourceSettlement::RetryWithdrawn,
            ..
        }),
    ) = (first, &replay)
    else {
        panic!(
            "expected settled native input and replay: {} / {}",
            submission_context(first),
            submission_context(&replay)
        );
    };
    assert_eq!(a, b);
    assert_eq!(fixture.binding().await.unwrap(), Some(before));
    assert_eq!(fixture.model_requests(), count);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_admin_parent_binds_exact_session_child_and_replays_once() {
    let fixture = configured().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let first = fixture.submit().await;
        fixture.finish(&first).await.unwrap();
        let binding = fixture.binding().await.unwrap().unwrap();
        let WorkAuthorityBasis::GrantLineage { lineage } = &binding.candidate().authority_basis
        else {
            panic!("configured child missing");
        };
        assert_eq!(lineage.len(), 2);
        assert_eq!(lineage[0], parent(&fixture));
        let route = fixture
            .source
            .route(&fixture.submission.original.route)
            .unwrap();
        let resolved = fixture
            .source
            .owner
            .grants()
            .resolve_lineage(
                lineage,
                &route.logical_executor,
                route.represented_subject.as_ref(),
            )
            .unwrap();
        let mut expected = crate::installed_host::application_operations::parent_restrictions(
            crate::installed_host::application_operations::find_and_install(
                &route.grant_caller,
                &route.logical_executor,
                &route.grant_audience,
                &fixture.session,
            ),
        );
        expected.delegation_depth =
            meerkat_authorization_contracts::constraints::DelegationDepth::remaining(0);
        assert_eq!(resolved.restrictions(), &expected);
        assert_eq!(
            binding.candidate().target.context.as_str(),
            fixture.session.to_string()
        );
        assert_eq!(fixture.model_requests(), 1);
        unchanged_replay(&fixture, &first).await;
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_or_revoked_configured_parent_binds_controller_only() {
    for revoked in [false, true] {
        let fixture = if revoked {
            configured().await
        } else {
            FirstFixture::new(true).await
        };
        let result = AssertUnwindSafe(Box::pin(async {
            if revoked {
                let parent = parent(&fixture);
                let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
                fixture.source.owner.grants().revoke(&parent.root_authority, &parent, &mut custody).unwrap();
            }
            let first = fixture.submit().await;
            fixture.finish(&first).await.unwrap();
            let binding = fixture.binding().await.unwrap().unwrap();
            assert!(matches!(&binding.candidate().authority_basis,
                WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } if targets.is_empty()));
            let child = EvidenceId::new(format!("original-configured-setup-{}", fixture.source.key(&fixture.submission.original).unwrap().id)).unwrap();
            assert!(fixture.source.owner.grants().issued_reference(&child).unwrap().is_none());
            unchanged_replay(&fixture, &first).await;
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoking_bound_configured_parent_refuses_without_substitution() {
    let fixture = configured().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let first = fixture.submit().await;
        fixture.finish(&first).await.unwrap();
        let before = fixture.binding().await.unwrap().unwrap();
        let count = fixture.model_requests();
        let native = fixture
            .lifetime()
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap();
        let owner = crate::installed_host::operator_setup::OperatorSetup::from_local_gateway(
            fixture.lifetime().clone(),
            fixture.store.clone(),
            fixture.source.owner.clone(),
        )
        .unwrap();
        let replacement = owner
            .configure_agent_setup(
                &crate::installed_host::operator_setup::AgentSetupCapabilityRequest {
                    helper: None,
                    member: native.members[0].member.clone(),
                    route: native.routes[0].id.clone(),
                    issuance_id: EvidenceId::new("different-configured-parent").unwrap(),
                    helper_targets: vec![],
                },
            )
            .unwrap();
        // A real independently issued parent cannot retarget an old child or
        // replace this already bound row. This candidate is rejection-only.
        let mut changed = before.candidate().clone();
        let WorkAuthorityBasis::GrantLineage { lineage } = &mut changed.authority_basis else {
            panic!("setup lineage missing");
        };
        lineage[0] = replacement.parent;
        let changed = InputAuthorityAssociation::new(changed).unwrap();
        assert!(
            fixture
                .source
                .persist_or_observe_binding(&fixture.submission.original, &changed, true)
                .await
                .is_err()
        );
        assert_eq!(fixture.binding().await.unwrap(), Some(before.clone()));
        let parent = parent(&fixture);
        let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
        fixture
            .source
            .owner
            .grants()
            .revoke(&parent.root_authority, &parent, &mut custody)
            .unwrap();
        drop(custody);
        assert!(matches!(
            fixture.submit().await,
            Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)
        ));
        assert_eq!(fixture.binding().await.unwrap(), Some(before));
        assert_eq!(fixture.model_requests(), count);
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issued_child_survives_source_write_failure_without_new_id_or_session() {
    let fixture = configured().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let key = fixture.source.key(&fixture.submission.original).unwrap();
        let before = fixture.store.get(&key).await.unwrap().unwrap();
        let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
        // Definite precommit source failure, not a lost committed reply.
        sql.execute_batch(
            "CREATE TRIGGER refuse_configured_bind BEFORE UPDATE OF data ON toolkit_records \
            WHEN json_extract(NEW.data, '$.body.final_association') IS NOT NULL \
            BEGIN SELECT RAISE(FAIL, 'configured source bind failure'); END;",
        )
        .unwrap();
        assert!(matches!(
            fixture.submit().await,
            Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)
        ));
        let selected = fixture.store.get(&key).await.unwrap().unwrap();
        let mut expected = fixture.source.decode_record(&before, &fixture.submission.original).unwrap();
        assert!(expected.configured_setup.is_none());
        // First binding retains this exact host choice before child issuance.
        // The trigger refuses the later association write, not that selection.
        expected.configured_setup = Some(configured_setup::ConfiguredSetupSelection {
            capability: fixture.lifetime().selected_policy().native_host.as_ref().unwrap()
                .members[0].setup_capability.as_ref().unwrap().clone(),
            session_id: fixture.session.clone(),
        });
        assert_eq!(selected.key, before.key);
        assert_eq!(selected.revision, before.revision + 1);
        assert_eq!(selected.data, encode(expected).unwrap());
        assert_eq!(fixture.model_requests(), 0);
        let child_id = EvidenceId::new(format!("original-configured-setup-{}", key.id)).unwrap();
        let issued = fixture
            .source
            .owner
            .grants()
            .issued_reference(&child_id)
            .unwrap()
            .expect("native child issued before failed source CAS");
        let record = fixture
            .source
            .decode_record(&selected, &fixture.submission.original)
            .unwrap();
        assert!(
            fixture
                .source
                .configured_setup_lineage(
                    &record,
                    &fixture.submission.original,
                    &SessionId::new(),
                    None
                )
                .await
                .is_err(),
            "same child ID cannot authorize a different session"
        );
        assert_eq!(
            fixture
                .source
                .owner
                .grants()
                .issued_reference(&child_id)
                .unwrap(),
            Some(issued.clone())
        );
        assert_eq!(fixture.store.get(&key).await.unwrap().unwrap(), selected);
        sql.execute_batch("DROP TRIGGER refuse_configured_bind;")
            .unwrap();
        let first = fixture.submit().await;
        fixture.finish(&first).await.unwrap();
        let binding = fixture.binding().await.unwrap().unwrap();
        let WorkAuthorityBasis::GrantLineage { lineage } = &binding.candidate().authority_basis
        else {
            panic!("setup child missing");
        };
        assert_eq!(lineage, &[parent(&fixture), issued]);
        unchanged_replay(&fixture, &first).await;
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_configured_original_never_issues_or_admits() {
    let fixture = configured().await;
    let result = AssertUnwindSafe(Box::pin(async {
        fixture
            .source
            .cancel_original_operation(&fixture.submission.original)
            .await
            .unwrap();
        assert!(matches!(
            fixture.submit().await,
            Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)
        ));
        assert!(fixture.binding().await.unwrap().is_none());
        let child = EvidenceId::new(format!(
            "original-configured-setup-{}",
            fixture.source.key(&fixture.submission.original).unwrap().id
        ))
        .unwrap();
        assert!(
            fixture
                .source
                .owner
                .grants()
                .issued_reference(&child)
                .unwrap()
                .is_none()
        );
        assert_eq!(fixture.inputs().await.unwrap(), 0);
        assert_eq!(fixture.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unselected_admin_parent_never_widens_bound_original() {
    let fixture = FirstFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let first = fixture.submit().await;
        fixture.finish(&first).await.unwrap();
        let before = fixture.binding().await.unwrap().unwrap();
        assert!(matches!(&before.candidate().authority_basis,
            WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } if targets.is_empty()));
        let owner = crate::installed_host::operator_setup::OperatorSetup::from_local_gateway(
            fixture.lifetime().clone(), fixture.store.clone(), fixture.source.owner.clone()).unwrap();
        let native = fixture.lifetime().selected_policy().native_host.as_ref().unwrap();
        let request = crate::installed_host::operator_setup::AgentSetupCapabilityRequest {
            helper: None,
            member: native.members[0].member.clone(), route: native.routes[0].id.clone(),
            issuance_id: EvidenceId::new("not-compiled-acquisition-parent").unwrap(),
            helper_targets: vec![],
        };
        let issued = owner.configure_agent_setup(&request).unwrap();
        assert_eq!(owner.configure_agent_setup(&request).unwrap(), issued);
        // Issuance is not compiler publication. The exact already-bound row
        // remains authoritative even after the native parent exists.
        unchanged_replay(&fixture, &first).await;
        assert_eq!(fixture.binding().await.unwrap(), Some(before));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[path = "configured_setup_store.rs"]
mod store_fault;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn committed_binding_and_lost_read_recover_exact_native_child() {
    let captured_fault = Arc::new(Mutex::new(None));
    let capture = captured_fault.clone();
    let fixture = Box::pin(FirstFixture::new_selected_configuration_with_source(
        true, false, true, None, None, true,
        Some(Box::new(move |store| {
            let fault = store_fault::LostReplyStore::new(store);
            *capture.lock().unwrap() = Some(fault.clone());
            fault
        })),
    )).await;
    let fault = captured_fault.lock().unwrap().take().expect("fault installed before source sharing");
    let result = AssertUnwindSafe(Box::pin(async {
        // The native policy and binder retain the same source adapter. Only
        // its storage observation is faulted; the canonical SQLite owner stays.
        // Observe canonical storage through the same adapter's decoder without
        // consuming the one injected read failure intended for its next submit.
        let binding = || async {
            let record = tokio::time::timeout(WAIT, fixture.store.get(
                &fixture.source.key(&fixture.submission.original).unwrap(),
            )).await.unwrap().unwrap().unwrap();
            fixture.source.decode_record(&record, &fixture.submission.original).unwrap().final_association
        };
        let invoke = || async {
            tokio::time::timeout(
                WAIT,
                fixture.source.submit_authenticated_original(fixture.submission.clone()),
            )
            .await
            .unwrap()
        };
        assert!(matches!(
            invoke().await,
            Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)
        ));
        assert!(fault.committed.load(Ordering::SeqCst));
        let saved = binding().await.unwrap();
        let WorkAuthorityBasis::GrantLineage { lineage } = &saved.candidate().authority_basis
        else {
            panic!("committed setup child missing");
        };
        let exact_child = lineage[1].clone();
        assert_eq!(fixture.model_requests(), 0);
        // A failed read before native entry is an outer error; the same
        // fault inside the native binder is a projected unavailable outcome.
        // Only that single evidenced fault permits one same-request recovery.
        let mut recovered = invoke().await;
        if matches!(recovered,
            Err(PrivateOriginalError::Unavailable)
                | Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable))
        {
            assert_eq!(fault.failed_reads.load(Ordering::SeqCst), 1);
            assert_eq!(binding().await, Some(saved.clone()));
            assert_eq!(fixture.source.owner.grants()
                .issued_reference(&exact_child.grant_id).unwrap(), Some(exact_child.clone()));
            assert_eq!(fixture.model_requests(), 0);
            recovered = invoke().await;
        }
        let recovered = recovered.map_err(|error| error.to_string());
        fixture.finish(&recovered).await.unwrap();
        assert_eq!(fault.failed_reads.load(Ordering::SeqCst), 1);
        assert_eq!(binding().await, Some(saved));
        assert_eq!(
            fixture
                .source
                .owner
                .grants()
                .issued_reference(&exact_child.grant_id)
                .unwrap(),
            Some(exact_child)
        );
        assert_eq!(fixture.model_requests(), 1);
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

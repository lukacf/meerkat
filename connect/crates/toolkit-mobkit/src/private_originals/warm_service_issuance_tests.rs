//! Actual configured helper capture, protected approval and canonical v2 source.
//! The extra login is a mechanical lifetime as in helper_entry_native_tests;
//! this does not qualify Console authentication, service attachment or B entry.
use super::*;
use crate::installed_host::acquisition::{AcquisitionBootstrap, InstalledServiceTaskSource};
use crate::installed_host::tool_policy::ServiceToolPolicySelection;
use meerkat_core::{ApprovalDecision, ApprovalPrincipalId, ApprovalService};

fn request(fixture: &Fixture) -> meerkat_core::ApprovalRequest {
    use meerkat_core::{
        ApprovalActionKind, ApprovalOwnerRef, ApprovalProposedAction, ApprovalResourceId,
        ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk,
    };
    let body = serde_json::json!({"fixture": "warm-service-first-bind"});
    meerkat_core::ApprovalRequest {
        requester: ApprovalPrincipalId::new("actual-setup-helper").unwrap(),
        owner: ApprovalOwnerRef::Session {
            session_id: fixture.session.clone(),
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("warm-service-first-bind"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "Review the public service and optional warm setup scope".into(),
            body: Some(body.clone()),
        },
        risk: ApprovalRisk::Medium,
        request_body: Some(body),
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into(),
        expires_at: Some((std::time::SystemTime::now() + Duration::from_secs(60)).into()),
        metadata: Default::default(),
        request_provenance: None,
    }
}

async fn approved_source(
    fixture: &Fixture,
    script: &Arc<acquisition_denial::Script>,
    report: &Arc<Report>,
) -> PrivateOriginalRef {
    let _context = report.take_context().await;
    let owner = script.installed_owner();
    let mut bootstrap = AcquisitionBootstrap::selected(owner.clone());
    assert!(Arc::ptr_eq(
        &bootstrap.prepare(None, None).unwrap().unwrap(),
        &owner
    ));
    let bootstrap = Arc::new(Mutex::new(bootstrap));
    let task = report
        .prepare_task(&InstalledServiceTaskSource::new(&bootstrap), ReportPreparation::PublicService)
        .await
        .unwrap();
    assert!(task.has_configured_setup());
    assert!(task.console_login().is_some());
    let mut configuration =
        meerkat_core::McpServerConfig::stdio("warm_x", "not-executed", vec![], Default::default());
    configuration.access = Some(meerkat_core::McpServerAccess::allowing(["read"]));
    configuration
        .tool_names
        .insert("read".into(), "warm_x_read".into());
    let prepared = task
        .prepare_public_batch(
            vec![PublicServiceBatchEntry {
                configuration,
                operations: [Id::new("read").unwrap()].into(),
                request: request(fixture),
            }],
            Some(60_000),
            request(fixture),
            ServiceToolPolicySelection {
                tools: vec![crate::installed_host::tool_policy::ServiceToolInvocation {
                    name: "warm_x_read".into(),
                    consequence: meerkat_core::CompiledToolConsequence::R0,
                }],
                catalog: Some(meerkat_core::CompiledToolConsequence::R0),
            },
        )
        .await
        .unwrap();
    let body = &prepared.request().proposed_action.body.as_ref().unwrap()["access_request"];
    let fresh = body["service_instruction"]["fresh_original"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        body["configured_setup"]["processors"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        body["configured_setup"]["sessions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let approvals = ApprovalService::new();
    let pending = approvals.request(prepared.request().clone()).unwrap();
    let approved = approvals
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("fixture-exact-warm-review").unwrap(),
            None,
            None,
        )
        .unwrap();
    assert!(prepared.approve(approved, || true).await.unwrap());
    let lookup = PrivateOriginalRef {
        route: fixture.original_reference().route.clone(),
        original: EvidenceId::new(fresh).unwrap(),
        event_sha256: String::new(),
    };
    let stored = fixture
        .store
        .get(&fixture.source.key(&lookup).unwrap())
        .await
        .unwrap()
        .unwrap();
    let record = fixture.source.decode_stored(&stored).unwrap();
    let approved = record.approved_task.as_ref().unwrap();
    assert!(approved.setup().is_some());
    // Exercise strict decoding using the actual retained selection. These
    // passive round trips supply no native authority.
    let wire = serde_json::to_string(approved).unwrap();
    let decoded: crate::private_originals::service_approved_input::ApprovedTaskLink =
        serde_json::from_str(&wire).unwrap();
    assert!(decoded == *approved);
    assert_eq!(serde_json::to_string(&decoded).unwrap(), wire);
    let value = serde_json::to_value(approved).unwrap();
    assert_eq!(value["kind"], "public_services_with_setup_v2");
    assert_eq!(value["members"].as_array().unwrap().len(), 1);
    for variant in ["missing", "null", "legacy", "unknown"] {
        let mut malformed = value.clone();
        match variant {
            "missing" => {
                malformed.as_object_mut().unwrap().remove("setup");
            }
            "null" => malformed["setup"] = serde_json::Value::Null,
            "legacy" => malformed["kind"] = "public_services_v1".into(),
            _ => malformed["setup"]["unexpected"] = true.into(),
        }
        assert!(
            serde_json::from_value::<
                crate::private_originals::service_approved_input::ApprovedTaskLink,
            >(malformed)
            .is_err(),
            "malformed v2 selection accepted: {variant}"
        );
    }
    let duplicate = format!("{{\"setup\":{},{}", value["setup"], &wire[1..]);
    assert!(
        serde_json::from_str::<crate::private_originals::service_approved_input::ApprovedTaskLink>(
            &duplicate
        )
        .is_err()
    );
    assert!(
        record.final_association.is_none()
            && record.bound_service_input.is_none()
            && record.service_admission.is_none()
    );
    assert!(record.service_setup_children.is_empty());
    PrivateOriginalRef {
        event_sha256: digest(&canonical_json(&record.event).unwrap()),
        ..lookup
    }
}

async fn partial_case(mismatched_child: bool) {
    let script = acquisition_denial::Script::paused_for_helper_entry();
    let report = Report::new(script.tools().result_tool());
    let (fixture, history) = Fixture::for_warm_helper_report(script.clone(), report.clone()).await;
    let (_pipe, _logins, login) = super::super::helper_entry_native::enrolled_login();
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT, Box::pin(async {
        let first = fixture.submit().await;
        script.first_response_waiting().await;
        let a = fixture.binding().await.unwrap().unwrap();
        super::super::helper_entry_native::retain_actual_a_login(&fixture, &a, &login);
        script.release_first_response();
        fixture.finish(&first).await.unwrap();
        let Ok(PrivateOriginalSubmission::Admitted { input_id, .. }) = &first else {
            panic!("actual A admission required");
        };
        // The live driver can archive a completed row. Observe the exact
        // canonical A input and its association through the retained store.
        // The outer WAIT still bounds this whole partial-issuance control.
        let a_input = acquisition_approved::consumed_native_input(
            history.as_ref(), &fixture.session, &a, Some(input_id),
            tokio::time::Instant::now() + WAIT,
        ).await;
        let native_before = serde_json::to_value(vec![a_input]).unwrap();
        let b = approved_source(&fixture, &script, &report).await;
        let source = &fixture.source;
        let key = source.key(&b).unwrap();
        let before = fixture.store.get(&key).await.unwrap().unwrap();
        let record = source.decode_record(&before, &b).unwrap();
        let setup = record.approved_task.as_ref().unwrap().setup().unwrap().clone();
        let find_id = EvidenceId::new(format!("original-configured-setup-{}", key.id)).unwrap();
        let native = fixture.lifetime().selected_policy().native_host.as_ref().unwrap();
        let application = native.grant_administrator.clone();
        let route = source.route(&b.route).unwrap();
        let mut drifted = setup.clone();
        drifted.helper.audience = meerkat_authorization_contracts::constraints::AudienceRef::Principal {
            principal: route.logical_executor.clone(),
        };
        assert!(drifted.helper.audience != setup.helper.audience);
        assert!(source.issue_service_setup(&b, &drifted, Some(&login)).await.is_err(),
            "captured scope drift refuses rather than falling back to legacy");
        assert!(source.owner.grants().issued_reference(&find_id).unwrap().is_none());
        if mismatched_child {
            // A real native child with this ID but without the source-bound
            // attenuation must never become this source's cleanup authority.
            let mut wrong = source.owner.grants().resolve_lineage(
                std::slice::from_ref(&setup.configured.capability.parent), &application,
                route.represented_subject.as_ref()).unwrap().restrictions().clone();
            wrong.delegation_depth = meerkat_authorization_contracts::constraints::DelegationDepth::remaining(0);
            source.owner.grants().issue_child(&application, &setup.configured.capability.parent,
                find_id.clone(), route.logical_executor.clone(), wrong).unwrap();
        }
        let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
        if !mismatched_child {
            sql.execute_batch(&format!("CREATE TRIGGER refuse_warm_observation BEFORE UPDATE OF data ON toolkit_records \
                WHEN NEW.id = '{}' AND json_array_length(json_extract(NEW.data, '$.body.service_setup_children')) > 0 \
                BEGIN SELECT RAISE(ABORT, 'fixture lost child observation write'); END;", key.id.replace('\'', "''"))).unwrap();
        }
        assert!(source.issue_service_setup(&b, &setup, Some(&login)).await.is_err());
        let issued = source.owner.grants().issued_reference(&find_id).unwrap().unwrap();
        let unbound = fixture.store.get(&key).await.unwrap().unwrap();
        assert_eq!(before, unbound, "intent survives; a failed observation write is not proof of rollback");
        assert!(source.decode_record(&unbound, &b).unwrap().service_setup_children.is_empty());
        assert_eq!(serde_json::to_value(acquisition_approved::native_history(
            history.as_ref(), &fixture.session).await).unwrap(), native_before,
            "partial child issuance preserves the exact canonical A input and adds no B");
        if !mismatched_child { sql.execute_batch("DROP TRIGGER refuse_warm_observation;").unwrap(); }
        source.record_task_group_cancelled(&b).await.unwrap();
        assert!(!source.decode_record(&fixture.store.get(&source.key(fixture.original_reference()).unwrap()).await.unwrap().unwrap(),
            fixture.original_reference()).unwrap().operation_cancelled, "A remains unrelated to B-only cleanup");
        let operator = meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&native.realm).unwrap().operator;
        let members = native.members.iter().map(|member| member.member.clone()).collect::<Vec<_>>();
        let rows = source.source_status_page_with_recovery(None).await.unwrap();
        let read = source.read_service_recovery_page(&operator, &members, &rows.rows).await.unwrap();
        // Select the one real MCP parent by the canonical grant reference, not a presentation label.
        let grant = record.approved_task.as_ref().unwrap().grants()[1].clone();
        let row = read.rows.iter().find(|row| row.selection.reference == grant).unwrap();
        assert!(row.can_recover);
        let journal = fixture.source_path.with_file_name("grants.jsonl");
        let before_cleanup = std::fs::read(&journal).unwrap();
        let result = source.prepare_service_recovery(&read, row).await.unwrap().revoke().unwrap();
        if mismatched_child {
            assert!(result.uncertain, "unknown preexisting child cannot be claimed as cleanup");
            assert!(source.decode_record(&fixture.store.get(&key).await.unwrap().unwrap(), &b).unwrap().service_setup_children.is_empty());
            let leaf = source.owner.grants().resolve_lineage(&[setup.configured.capability.parent.clone(), issued],
                &route.logical_executor, route.represented_subject.as_ref()).unwrap();
            leaf.check_current().unwrap();
        } else {
            assert_eq!(result.parent, "not_current", "reconciliation CAS invalidates the original offer");
            assert_eq!(std::fs::read(&journal).unwrap(), before_cleanup);
            let reconciled = source.decode_record(&fixture.store.get(&key).await.unwrap().unwrap(), &b).unwrap();
            assert_eq!(reconciled.service_setup_children.len(), 1);
            let rows = source.source_status_page_with_recovery(None).await.unwrap();
            let fresh = source.read_service_recovery_page(&operator, &members, &rows.rows).await.unwrap();
            let row = fresh.rows.iter().find(|row| row.selection.reference == grant).unwrap();
            let result = source.prepare_service_recovery(&fresh, row).await.unwrap().revoke().unwrap();
            assert_eq!(result.setup_children, Some("revoked_or_not_issued"));
            assert!(!result.uncertain);
            assert!(source.owner.grants().resolve_lineage(&[setup.configured.capability.parent.clone(), issued],
                &route.logical_executor, route.represented_subject.as_ref()).is_err());
            let revoked = std::fs::read(&journal).unwrap();
            let rows = source.source_status_page_with_recovery(None).await.unwrap();
            let repeat = source.read_service_recovery_page(&operator, &members, &rows.rows).await.unwrap();
            let row = repeat.rows.iter().find(|row| row.selection.reference == grant).unwrap();
            assert!(!source.prepare_service_recovery(&repeat, row).await.unwrap().revoke().unwrap().uncertain);
            assert_eq!(std::fs::read(&journal).unwrap(), revoked, "observed already-revoked children reconcile idempotently");
        }
        source.owner.grants().resolve_lineage(std::slice::from_ref(&setup.configured.capability.parent),
            &application, route.represented_subject.as_ref()).unwrap().check_current().unwrap();
        for lineage in a.candidate().authority_basis.grant_lineages() {
            source.owner.grants().resolve_lineage(lineage, &route.logical_executor, route.represented_subject.as_ref())
                .unwrap().check_current().unwrap();
        }
        assert_eq!(serde_json::to_value(acquisition_approved::native_history(
            history.as_ref(), &fixture.session).await).unwrap(), native_before,
            "cleanup preserves the exact canonical A input and adds no B");
        let after = source.decode_record(&fixture.store.get(&key).await.unwrap().unwrap(), &b).unwrap();
        assert!(after.operation_cancelled && after.final_association.is_none()
            && after.bound_service_input.is_none() && after.service_admission.is_none(),
            "B remains cancelled before binding or native input admission");
    }))).catch_unwind().await;
    script.release_first_response();
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("warm first-bind fixture exceeded its bounded deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warm_partial_child_write_requires_fresh_recovery_offer_and_preserves_shared_parents() {
    partial_case(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warm_preexisting_mismatched_child_stays_uncertain_and_unrevoked() {
    partial_case(true).await;
}

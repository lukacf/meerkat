//! Canonical source/WorkGraph controls. No live native admission or auth flow.
use super::*;

fn changed_facts(fixture: &Fixture, facts: HelperAdmissionFacts) -> AdmittedHelperWork {
    AdmittedHelperWork {
        owner: fixture.owner.clone(), source_scope: fixture.admitted.source_scope.clone(),
        facts, contributor: fixture.admitted.contributor.clone(),
        contributors: fixture.admitted.contributors.clone(),
    }
}

#[tokio::test]
async fn qualified_origin_follows_designated_source_not_recursive_order() {
    let fixture = Fixture::with_ancestor().await;
    let claims = fixture.admitted.contributor.association().candidate();
    let origin = fixture.admitted.origin().clone();
    assert!(origin.realm == claims.ingress_namespace.realm);
    assert!(origin.source_owner == claims.ingress_namespace.ingress);
    assert!(origin.original_authority == claims.original_work.authority);
    assert_eq!(origin.original.original, claims.original_work.work);
    assert_ne!(origin.original.original,
        fixture.admitted.contributors[0].association().candidate().original_work.work);
    let (binding, _) = super::report_tests::issued(&fixture).await;
    let source = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding.clone()).await.unwrap();
    let policy = source.policy_source().unwrap();
    assert!(policy.origin == origin);
    assert_eq!(policy.contributors, fixture.admitted.facts.contributors);
    let recovered = fixture.owner.observe_helper_binding(&fixture.store, &fixture.admitted).await.unwrap().unwrap();
    assert!(recovered == binding);
    let replay = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, recovered).await.unwrap();
    assert!(replay.policy_source().unwrap().origin == origin);
    assert_eq!(source.source_record, replay.source_record);
}

#[tokio::test]
async fn staged_origin_and_recursive_membership_cannot_change_before_issuance() {
    let fixture = Fixture::with_ancestor().await;
    let candidate = fixture.candidate();
    let (controller, report) = fixture.owner.retain_helper_grant_intents(
        &fixture.store, &fixture.admitted, candidate.clone()).await.unwrap();
    let report = report.unwrap();
    let saved = fixture.row().await.unwrap();
    for change in 0..7 {
        let mut facts = fixture.admitted.facts.clone();
        match change {
            0 => facts.origin.realm = meerkat_core::RealmId::parse("different-realm").unwrap(),
            1 => facts.origin.source_owner.authority = crate::original_ingress::tests::principal("different-source"),
            2 => facts.origin.source_owner.namespace = "different-ingress".into(),
            3 => facts.origin.original.original = fixture.admitted.contributors[0]
                .association().candidate().original_work.work.clone(),
            4 => facts.origin.original.route = toolkit_package::Id::new("different-route").unwrap(),
            5 => facts.origin.original.event_sha256 = "1".repeat(64),
            6 => facts.origin.original_authority = crate::original_ingress::tests::principal("different-original-source"),
            _ => unreachable!(),
        }
        let changed = changed_facts(&fixture, facts);
        assert!(fixture.owner.retain_helper_grant_intents(&fixture.store, &changed, candidate.clone()).await.is_err());
        assert_eq!(fixture.row().await.unwrap(), saved);
    }
    // Keep facts and handles mutually consistent but omit the recorded
    // ancestor. Native source membership must still remain complete.
    let mut changed = changed_facts(&fixture, fixture.admitted.facts.clone());
    changed.contributors.remove(0);
    changed.facts.contributors.remove(0);
    assert!(fixture.owner.retain_helper_grant_intents(&fixture.store, &changed, candidate.clone()).await.is_err());
    // Choosing the ancestor as the primary cannot retarget a staged origin.
    let mut changed = changed_facts(&fixture, fixture.admitted.facts.clone());
    changed.contributor = changed.contributors[0].clone();
    assert!(fixture.owner.retain_helper_grant_intents(&fixture.store, &changed, candidate).await.is_err());
    assert_eq!(fixture.row().await.unwrap(), saved);
    for id in [&controller.id, &report.id] {
        assert!(fixture.owner.grants.issued_reference(id).unwrap().is_none());
    }
    fixture.assert_no_source();
}

#[tokio::test]
async fn qualified_origin_is_frozen_in_the_pre_admission_work_request() {
    let fixture = Fixture::with_ancestor().await;
    let service = WorkGraphService::with_namespace_grant(
        Arc::new(SqliteWorkGraphStore::open(fixture._directory.path().join("work.sqlite3")).unwrap()),
        WorkGraphNamespaceGrant::new("test-realm", "setup").unwrap()).unwrap();
    let exact: CreateWorkItemRequest = serde_json::from_value(fixture.admitted.facts.admitted_request.clone()).unwrap();
    assert!(matches!(service.create_idempotent(fixture.admitted.facts.admission.key.clone(), exact.clone()).await.unwrap(),
        WorkAdmissionOutcome::Replayed(_)));
    for change in 0..4 {
        let mut origin = fixture.admitted.origin().clone();
        match change {
            0 => origin.original.route = toolkit_package::Id::new("different-route").unwrap(),
            1 => origin.original.event_sha256 = "1".repeat(64),
            2 => origin.original.original = fixture.admitted.contributors[0]
                .association().candidate().original_work.work.clone(),
            3 => origin.original_authority = crate::original_ingress::tests::principal("different-original-source"),
            _ => unreachable!(),
        }
        let mut request = exact.clone();
        assert_eq!(request.external_refs.pop().unwrap().kind, CORRELATION);
        bind_work_request(&mut request, &fixture.admitted.facts.operation, &origin,
            &fixture.admitted.contributors, fixture.admitted.selection()).unwrap();
        assert!(matches!(service.create_idempotent(fixture.admitted.facts.admission.key.clone(), request).await.unwrap(),
            WorkAdmissionOutcome::Conflict { .. }));
    }
    assert!(fixture.row().await.is_none());
    fixture.assert_no_source();
}

#[tokio::test]
async fn helper_source_schema_never_infers_original_authority_from_ingress() {
    let fixture = Fixture::with_ancestor().await;
    let candidate = fixture.candidate();
    let (controller, report) = fixture.owner.retain_helper_grant_intents(
        &fixture.store, &fixture.admitted, candidate.clone()).await.unwrap();
    let saved = fixture.row().await.unwrap();
    assert_eq!(saved.data.version, 3);
    assert!(HelperSourceRecord::decode(&saved).is_ok());
    for change in 0..2 {
        // Negative decode only. The actual stored row, source and native
        // authority remain unchanged; each altered record has a valid digest.
        let mut changed = saved.clone();
        if change == 0 {
            changed.data.version = 2;
        } else {
            changed.data.body["facts"]["origin"].as_object_mut().unwrap()
                .remove("original_authority").unwrap();
        }
        changed.content_digest = changed.data.digest().unwrap();
        assert!(HelperSourceRecord::decode(&changed).is_err());
    }
    let replay = fixture.owner.retain_helper_grant_intents(
        &fixture.store, &fixture.admitted, candidate).await.unwrap();
    assert!(replay == (controller.clone(), report.clone()));
    assert_eq!(fixture.row().await.unwrap(), saved);
    assert!(fixture.owner.grants.issued_reference(&controller.id).unwrap().is_none());
    assert!(fixture.owner.grants.issued_reference(&report.unwrap().id).unwrap().is_none());
    fixture.assert_no_source();
}

#[tokio::test]
async fn recursive_source_cancellation_prevents_both_helper_intents() {
    let fixture = Fixture::with_ancestor().await;
    let candidate = fixture.candidate();
    let report = candidate.report.as_ref().unwrap().native(&fixture.admitted.facts).unwrap();
    fixture.owner.originals.lock().unwrap().iter_mut().filter_map(OriginalRecord::bound_mut)
        .find(|held| Arc::ptr_eq(&held.retained, &fixture.admitted.contributors[0])).unwrap()
        .operation = OriginalOperationDisposition::CancellationPending;
    assert!(matches!(fixture.owner.retain_helper_grant_intents(&fixture.store, &fixture.admitted, candidate.clone()).await,
        Err(OriginalWriterError::Unavailable)));
    assert!(fixture.owner.grants.issued_reference(&candidate.id).unwrap().is_none());
    assert!(fixture.owner.grants.issued_reference(&report.id).unwrap().is_none());
    assert!(fixture.row().await.is_none());
    fixture.assert_no_source();
}

#[tokio::test]
async fn report_child_uses_qualified_origin_while_binding_keeps_actual_helper_target() {
    use crate::installed_host::application_operations;
    let fixture = Fixture::with_ancestor().await;
    let candidate = fixture.candidate();
    let (binding, report) = super::report_tests::issued(&fixture).await;
    let helper = &fixture.admitted.selection().helper;
    let exact = application_operations::setup_report_for_origin(
        &fixture.admitted.selection().grant_caller, &helper.logical_executor,
        &binding.target.audience, fixture.admitted.origin(),
    ).unwrap();
    let expected = application_operations::parent_restrictions(exact.clone());
    assert_eq!(report.requested.resource_domains, expected.resource_domains);
    assert_eq!(report.requested.actions, expected.actions);
    let lineage = crate::installed_host::acquisition::ConfiguredHelperReportCapability::selected_lineage(
        &binding.authority_basis).unwrap();
    let grant = fixture.owner.grants.resolve_controller_lineage(
        lineage, &helper.logical_executor, helper.represented_subject.as_ref()).unwrap();
    assert!(in_bounds(grant.restrictions(), &exact));
    // The origin is independent of later native session allocation. An
    // already staged binding still cannot be retargeted to that other session.
    let mut later_target = binding.target.clone();
    let later_session = meerkat_core::SessionId::new();
    later_target.context = EvidenceId::new(later_session.to_string()).unwrap();
    later_target.logical_runtime = EvidenceId::new(
        meerkat_runtime::LogicalRuntimeId::for_session(&later_session).to_string()).unwrap();
    let later = report::HelperReportIntent::for_target(&fixture.admitted, &later_target).unwrap().unwrap();
    assert!(later == *candidate.report.as_ref().unwrap());
    let source = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding.clone()).await.unwrap();
    let mut changed_binding = binding.clone(); changed_binding.target = later_target;
    assert!(fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, changed_binding).await.is_err());
    let replay = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding).await.unwrap();
    assert_eq!(replay.source_record, source.source_record);
}

#[tokio::test]
async fn report_bounds_exclude_other_qualified_origins_account_and_acquisition() {
    use crate::installed_host::application_operations;
    let fixture = Fixture::with_ancestor().await;
    let (binding, _) = super::report_tests::issued(&fixture).await;
    let selected = fixture.admitted.selection();
    let helper = &selected.helper;
    let lineage = crate::installed_host::acquisition::ConfiguredHelperReportCapability::selected_lineage(
        &binding.authority_basis).unwrap();
    let grant = fixture.owner.grants.resolve_controller_lineage(
        lineage, &helper.logical_executor, helper.represented_subject.as_ref()).unwrap();
    for field in 0..7 {
        let mut origin = fixture.admitted.origin().clone();
        match field {
            0 => origin.realm = meerkat_core::RealmId::parse("other-realm").unwrap(),
            1 => origin.source_owner.authority = crate::original_ingress::tests::principal("other-source"),
            2 => origin.source_owner.namespace = "other-ingress".into(),
            3 => origin.original.original = fixture.admitted.contributors[0].association().candidate().original_work.work.clone(),
            4 => origin.original.route = toolkit_package::Id::new("other-route").unwrap(),
            5 => origin.original.event_sha256 = "f".repeat(64),
            6 => origin.original_authority = crate::original_ingress::tests::principal("other-original-source"),
            _ => unreachable!(),
        }
        let other = application_operations::setup_report_for_origin(
            &selected.grant_caller, &helper.logical_executor, &binding.target.audience, &origin,
        ).unwrap();
        assert!(!in_bounds(grant.restrictions(), &other));
    }
    let account = application_operations::setup_account_request_for_origin(
        &selected.grant_caller, &helper.logical_executor, &binding.target.audience, fixture.admitted.origin(),
    ).unwrap();
    let acquisition = application_operations::find_and_install(
        &selected.grant_caller, &helper.logical_executor, &binding.target.audience,
        &meerkat_core::SessionId::parse(binding.target.context.as_str()).unwrap(),
    );
    assert!(!in_bounds(grant.restrictions(), &account));
    assert!(!in_bounds(grant.restrictions(), &acquisition));
    let exact = application_operations::setup_report_for_origin(
        &selected.grant_caller, &helper.logical_executor, &binding.target.audience, fixture.admitted.origin(),
    ).unwrap();
    for field in 0..4 {
        let mut other = exact.clone();
        match field {
            0 => other.resource_domain.authority = crate::original_ingress::tests::principal("other-application"),
            1 => other.processor = meerkat_authorization_contracts::constraints::ProcessorRef::Principal {
                principal: crate::original_ingress::tests::principal("other-helper"),
            },
            2 => other.audience = meerkat_authorization_contracts::constraints::AudienceRef::Principal {
                principal: crate::original_ingress::tests::principal("other-audience"),
            },
            3 => other.action.action = "other-action".into(),
            _ => unreachable!(),
        }
        assert!(!in_bounds(grant.restrictions(), &other));
    }
}

#[tokio::test]
async fn historical_report_projection_refuses_an_intent_for_the_old_session_coordinate() {
    use crate::installed_host::application_operations;
    let fixture = Fixture::with_ancestor().await;
    let (binding, _) = super::report_tests::issued(&fixture).await;
    let mut source = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding.clone()).await.unwrap();
    assert!(source.policy_source().is_ok());
    let mut old = application_operations::parent_restrictions(application_operations::setup_report(
        &fixture.admitted.selection().grant_caller, &binding.logical_executor, &binding.target.audience,
        &meerkat_core::SessionId::parse(binding.target.context.as_str()).unwrap(),
    ));
    old.delegation_depth = DelegationDepth::remaining(0);
    // Negative decode only: do not mutate the stored row or native authority.
    // All selected lineages and the actual source remain the ones just bound.
    let mut changed = source.source_record.as_ref().clone();
    changed.data.body["controller"]["report"]["requested"] = serde_json::to_value(old).unwrap();
    changed.content_digest = changed.data.digest().unwrap();
    source.source_record = Arc::new(changed);
    assert!(source.policy_source().is_err());
    let unchanged = fixture.owner.observe_helper_binding(&fixture.store, &fixture.admitted).await.unwrap().unwrap();
    assert!(unchanged == binding);
}

fn in_bounds(bounds: &ExecutionRestrictions, values: &meerkat_authorization::policy::LocalOperationValues) -> bool {
    bounds.check_bounds(meerkat_authorization_contracts::constraints::OperationRestrictionValues {
        action: &values.action, resource_domain: &values.resource_domain,
        processor: &values.processor, audience: &values.audience, now_ms: 0,
    }).is_ok()
}

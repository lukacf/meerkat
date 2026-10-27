//! Joined trusted parent producer, ordinary helper child issuance and real
//! report leaf. Fixture configuration is not Forge publication or Console auth.
use meerkat_authorization_contracts::constraints::DomainRestriction;
use super::*;
use meerkat_authorization_contracts::{
    constraints::DelegationDepth,
    work_association::WorkAuthorityBasis,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deadline_recovery_worker_expires_waiting_setup_and_resumes_original() {
    use crate::installed_host::acquisition::recovery::AcquisitionRecovery;
    use meerkat_runtime::input_state::InputLifecycleState;
    let script = acquisition_denial::Script::for_expiring_setup(5_000);
    let report = Report::new(script.tools().result_tool());
    let (fixture, runtime_store) =
        Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    let recovery = AcquisitionRecovery::default();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout_at(deadline, Box::pin(async {
        let first = fixture.submit().await;
        let first_input = match &first {
            Ok(PrivateOriginalSubmission::Admitted { input_id, .. }) => input_id.clone(),
            _ => panic!("actual original admission required: {}", submission_context(&first)),
        };
        fixture.finish(&first).await.unwrap();
        let feedback = script.observations().0;
        assert_eq!(feedback.len(), 1);
        assert!(!feedback[0].is_error, "{feedback:?}");
        let context = report.take_context().await;
        let source = fixture.source.setup_result_source(&context, report.selection.get().unwrap()).await.unwrap();
        let key = source.operation.acquisition.clone();
        let before = fixture.store.get(&key).await.unwrap().unwrap();
        assert_eq!(before.data.body["state"]["lifecycle_phase"], "investigating");
        assert_eq!(before.data.body["state"]["terminal_revision"], 0);
        assert!(before.data.body["continuation_receipt"].is_null());
        // Start exactly the worker selected by HostGateway after native Ready.
        // No helper report, user request or direct expire call drives progress.
        recovery.start(recovery.prepare_start().unwrap(), script.installed_owner()).unwrap();
        let settled = loop {
            let row = fixture.store.get(&key).await.unwrap().unwrap();
            if !row.data.body["continuation_receipt"].is_null() { break row; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(settled.data.body["state"]["lifecycle_phase"], "expired");
        assert_eq!(settled.data.body["state"]["terminal_revision"], 2);
        assert_eq!(settled.data.body["request"], before.data.body["request"]);
        assert!(settled.data.body.get("helper_detail").is_none());
        assert!(settled.data.body.get("approved_service").is_none());
        assert!(report.feedback.lock().unwrap().is_empty(), "the held helper has not reported");
        let receipt = &settled.data.body["continuation_receipt"];
        assert_eq!(receipt["session"], fixture.session.to_string());
        let resumed_input = receipt["durable_input"]["id"].as_str().unwrap().to_owned();
        assert_ne!(resumed_input, first_input);
        loop {
            let history = acquisition_approved::native_history(runtime_store.as_ref(), &fixture.session).await;
            let resumed: Vec<_> = history.iter().filter(|row| row.state.input_id.to_string() == resumed_input).collect();
            assert!(resumed.len() <= 1);
            if resumed.first().is_some_and(|row| row.seed.phase == InputLifecycleState::Consumed) { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // A second normal sweep must observe the settled effect, not issue a
        // second terminal transition or a second original input.
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert!(fixture.store.get(&key).await.unwrap().unwrap() == settled);
        let history = acquisition_approved::native_history(runtime_store.as_ref(), &fixture.session).await;
        assert_eq!(history.iter().filter(|row| row.state.input_id.to_string() == resumed_input).count(), 1);
        assert_eq!(script.observations().2, 1, "no reacquisition");
    }))).catch_unwind().await;
    recovery.stop();
    recovery.join().await.unwrap();
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("continuous recovery did not resume Original before the fixture deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_report_parent_reaches_actual_helper_report_and_consumed_input() {
    let script = acquisition_denial::Script::for_approved();
    let report = Report::new(script.tools().result_tool());
    let (fixture, runtime_store) =
        Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    let deadline = tokio::time::Instant::now() + WAIT;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout_at(deadline, Box::pin(async {
        let helper = AgentIdentity::parse(crate::installed_host::controller_first_input_fixture::HELPER).unwrap();
        let helper_member = MemberIdentity::from(helper.as_str());
        assert!(fixture.native.mob_handle().get_member(&helper_member).await.unwrap().is_none(),
            "the configured report producer must not require a materialized helper");
        assert!(fixture.source.helper_sources.lock().unwrap().is_empty());

        // The constructor called the actual stopped-host producer before any
        // runtime/session was created, then bound the installed acquisition
        // with this typed selection. It never issued a final report child.
        let selection = report.selection.get().unwrap();
        let capability = selection.report_capability.as_ref().unwrap();
        assert_eq!(capability.parent.len(), 1);
        assert_eq!(capability.parent[0].grant_id.as_str(), "configured-helper-report-parent");
        assert!(matches!(&selection.operation_basis,
            WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } if targets.is_empty()));
        let native = fixture._lifetime.selected_policy().native_host.as_ref().unwrap();
        assert_eq!(Some(&capability.parent[0].root_authority), native.remote_operator.as_ref());
        let parent = fixture.source.owner.grants().resolve_controller_lineage(
            &capability.parent, &selection.ingress_actor, selection.represented_subject.as_ref(),
        ).unwrap();
        let origin = fixture.source.setup_origin_for(&fixture.submission.original).unwrap();
        let mut exact_report = crate::installed_host::application_operations::parent_restrictions(
            crate::installed_host::application_operations::setup_report_for_origin(
                &native.grant_administrator, &selection.logical_executor,
                &native.members.iter().find(|member| member.member == selection.member).unwrap().audience,
                &origin,
            ).unwrap(),
        );
        exact_report.delegation_depth = DelegationDepth::remaining(0);
        let mut parent_bounds = exact_report.clone();
        parent_bounds.resource_domains = DomainRestriction::owned_by([native.grant_administrator.clone()]);
        parent_bounds.delegation_depth = DelegationDepth::remaining(1);
        assert_eq!(parent.restrictions(), &parent_bounds);
        parent.check_current().unwrap();
        drop(parent);

        let configured_a = native.members.iter().find(|member| member.member.member == MEMBER)
            .unwrap().setup_capability.as_ref().unwrap();
        assert_eq!(configured_a.helper_targets.len(), 3);
        assert!(capability.account_parent.is_some());
        assert!(capability.public_parent.is_some());
        let first = fixture.submit().await;
        let first_input_id = match &first {
            Ok(PrivateOriginalSubmission::Admitted { input_id, session_id, turn_ticket: Some(_), .. }) => {
                assert_eq!(session_id, &fixture.session.to_string()); input_id.clone()
            }
            _ => panic!("actual A admission required: {}", submission_context(&first)),
        };
        fixture.finish(&first).await.unwrap();
        // Observe the actual acquisition result before waiting for a helper
        // report. A refused acquisition has no helper context to publish.
        let (acquisition_feedback, planned, entered) = script.observations();
        assert_eq!(acquisition_feedback.len(), 1,
            "real in-turn acquisition feedback: {acquisition_feedback:?}");
        assert!(!acquisition_feedback[0].is_error, "{acquisition_feedback:?}");
        let acquisition_reply: serde_json::Value =
            serde_json::from_str(&acquisition_feedback[0].text_content()).unwrap();
        assert_eq!(acquisition_reply["created"], true, "{acquisition_reply}");
        assert_eq!(acquisition_reply["worker"]["state"], "input_admitted", "{acquisition_reply}");
        assert_eq!(acquisition_reply["worker"]["deduplicated"], false, "{acquisition_reply}");
        assert_eq!(acquisition_reply["worker"]["source_settlement"],
            "fresh_acceptance_promoted", "{acquisition_reply}");
        assert!(planned > 0, "actual registered leaf resolution required");
        assert_eq!(entered, 1, "actual enrolled leaf must enter exactly once");
        let context = report.take_context().await;
        let helper_session = fixture.native.mob_handle().get_member(&helper_member).await.unwrap().unwrap()
            .bridge_session_id().cloned().unwrap();
        assert_ne!(helper_session, fixture.session);
        let meerkat_core::exact_operation::OperationExecutionScope::RuntimeInput {
            canonical_input_id, ..
        } = context.work_authorization().unwrap().execution_scope()
        else { panic!("actual helper RuntimeInput authority required") };
        assert_eq!(context.interaction_lineage_id().map(|id| id.to_string()),
            Some(canonical_input_id.to_string()), "actual helper prompt retains its admitted input identity");
        assert!(context.work_authorization().is_some());
        assert!(context.retained_work().is_some());
        assert_eq!(context.origin_session_id(), Some(&helper_session));
        let source = fixture.source.setup_result_source(&context, selection).await.unwrap();
        // Optional transcript lineage is not authority. A lookup-only clone
        // keeps the same session, admitted work and retained native custody.
        let without_lineage = context.clone().with_runtime_identity(helper_session.clone(), None);
        let optional_source = fixture.source.setup_result_source(&without_lineage, selection).await.unwrap();
        assert_eq!(optional_source.helper, source.helper);
        assert_eq!(optional_source.source, source.source);
        assert_eq!(optional_source.contributors, source.contributors);
        drop(optional_source);
        drop(without_lineage);
        let original = fixture.binding().await.unwrap().unwrap();
        assert_eq!(source.contributors, vec![original.clone()]);
        assert_eq!(source.helper.candidate().target.context.as_str(), helper_session.to_string());
        assert_ne!(source.helper.candidate().original_work, original.candidate().original_work);
        let WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } =
            &source.helper.candidate().authority_basis else { panic!("actual targeted report, account and public children required") };
        assert_eq!(targets.len(), 3);
        let target = crate::installed_host::acquisition::ConfiguredHelperReportCapability::target();
        let lineage = source.helper.candidate().authority_basis.lineage_for(Some(&target)).unwrap();
        let a_lineage = original.candidate().authority_basis.lineage_for(Some(&target)).unwrap();
        assert_ne!(a_lineage, lineage, "A and helper retain independently constrained children");
        let a_report = fixture.source.owner.grants().resolve_lineage(
            a_lineage, &original.candidate().logical_executor,
            original.candidate().represented_subject.as_ref(),
        ).unwrap();
        assert_eq!(a_report.restrictions(), &exact_report);
        a_report.check_current().unwrap();
        assert_eq!(lineage.len(), capability.parent.len() + 1);
        assert!(lineage.starts_with(&capability.parent));
        let child = lineage.last().unwrap();
        assert_ne!(child.grant_id, capability.parent[0].grant_id);
        assert_eq!(fixture.source.owner.grants().issued_reference(&child.grant_id).unwrap().as_ref(), Some(child));
        let resolved = fixture.source.owner.grants().resolve_controller_lineage(
            lineage, &selection.logical_executor, selection.represented_subject.as_ref(),
        ).unwrap();
        assert_eq!(resolved.restrictions(), &exact_report, "exact qualified-origin report child only");
        resolved.check_current().unwrap();
        let public_target = crate::installed_host::acquisition::ConfiguredHelperReportCapability::public_target();
        let public_lineage = source.helper.candidate().authority_basis.lineage_for(Some(&public_target)).unwrap();
        let public_parent = capability.public_parent.as_ref().unwrap();
        assert!(public_lineage.starts_with(public_parent));
        assert_eq!(public_lineage.len(), public_parent.len() + 1);
        assert_ne!(public_lineage, lineage);
        let mut exact_public = crate::installed_host::application_operations::parent_restrictions(
            crate::installed_host::application_operations::setup_public_service_request_for_origin(
                &native.grant_administrator, &selection.logical_executor,
                &native.members.iter().find(|member| member.member == selection.member).unwrap().audience,
                &origin,
            ).unwrap(),
        );
        exact_public.delegation_depth = DelegationDepth::remaining(0);
        let public_resolved = fixture.source.owner.grants().resolve_controller_lineage(
            public_lineage, &selection.logical_executor, selection.represented_subject.as_ref(),
        ).unwrap();
        assert_eq!(public_resolved.restrictions(), &exact_public);
        public_resolved.check_current().unwrap();
        drop(public_resolved);
        let controller = &source.helper.candidate().controller_grant_lineage;
        assert!(controller.starts_with(&selection.controller_parent));
        assert_eq!(controller.len(), selection.controller_parent.len() + 1);
        assert_ne!(controller.as_slice(), lineage, "inference and report must remain separate grants");

        // Read the exact source row held by the actual input owner. The report
        // intent and its issued binding share this single canonical row.
        let retained_source = {
            let held = fixture.source.helper_sources.lock().unwrap();
            let mut matches = held.iter().filter(|held| held.original().association() == &source.helper);
            let exact = matches.next().unwrap().source_record().clone();
            assert!(matches.next().is_none());
            exact
        };
        let source_row = fixture.store.get(&retained_source.key).await.unwrap().unwrap();
        // Enrollment advances the canonical row without changing its Bound
        // facts. The result carries the current CAS condition, not the snapshot
        // retained before the first native input was enrolled.
        assert_eq!(retained_source.data.version, 3);
        assert_eq!(retained_source.revision, 2);
        assert_eq!(source_row.key, retained_source.key);
        assert_eq!(source_row.data.schema, retained_source.data.schema);
        assert_eq!(source_row.data.version, 5);
        assert!(source_row.revision > retained_source.revision);
        assert_eq!(source_row.content_digest, source_row.data.digest().unwrap());
        assert_eq!(source.source.key, source_row.key);
        assert_eq!(source.source.revision, source_row.revision);
        assert_eq!(source.source.content_digest, source_row.content_digest);
        let mut unchanged = source_row.data.body.clone();
        let attempt: crate::original_ingress::custody_attempt::CustodyAttempt =
            serde_json::from_value(unchanged.as_object_mut().unwrap()
                .remove("custody_attempt").unwrap()).unwrap();
        attempt.validate_binding(&source.helper).unwrap();
        assert_eq!(attempt.session(), &helper_session);
        assert_eq!(attempt.input_id().to_string(), canonical_input_id.to_string());
        assert!(attempt.decision() != crate::original_ingress::custody_attempt::CustodyDecision::Withdrawn);
        let retained_input: meerkat_runtime::Input = serde_json::from_value(
            unchanged.as_object_mut().unwrap().remove("bound_input").unwrap()).unwrap();
        assert!(attempt.matches_input(&retained_input).unwrap());
        assert_eq!(retained_input.header().authority_association.as_ref(), Some(&source.helper));
        assert_eq!(unchanged, retained_source.data.body);
        assert_eq!(source_row.data.body["state"]["phase"], "bound");
        assert_eq!(source_row.data.body["facts"]["selection"]["helper"], serde_json::to_value(selection).unwrap());
        assert_eq!(source_row.data.body["controller"]["report"]["id"], serde_json::to_value(&child.grant_id).unwrap());
        assert_eq!(source_row.data.body["controller"]["report"]["requested"], serde_json::to_value(&exact_report).unwrap());
        assert_eq!(source_row.data.body["controller"]["public"]["id"],
            serde_json::to_value(&public_lineage.last().unwrap().grant_id).unwrap());
        assert_eq!(source_row.data.body["controller"]["public"]["requested"], serde_json::to_value(&exact_public).unwrap());
        assert_eq!(source_row.data.body["state"]["binding"]["authority_basis"],
            serde_json::to_value(&source.helper.candidate().authority_basis).unwrap());
        assert_eq!(source_row.data.body["state"]["association"], serde_json::to_value(&source.helper).unwrap());

        // These additional copies exercise negative context/selection cases.
        // The real dispatch resumes with its unchanged call, plan and custody.
        let missing = context.clone().with_work_authorization(None);
        assert!(matches!(fixture.source.setup_result_source(&missing, selection).await,
            Err(PrivateOriginalError::Unavailable)));
        let wrong = context.clone().with_runtime_identity(fixture.session.clone(), None);
        assert!(matches!(fixture.source.setup_result_source(&wrong, selection).await,
            Err(PrivateOriginalError::Denied)));
        let mut unselected = selection.clone();
        unselected.report_capability = None;
        assert!(matches!(fixture.source.setup_result_source(&context, &unselected).await,
            Err(PrivateOriginalError::Denied)));
        let before = fixture.store.get(&source.operation.acquisition).await.unwrap().unwrap();
        assert!(before.data.body.get("helper_detail").is_none());
        drop(missing); drop(wrong); drop(context); drop(resolved);
        // The held report now has a real native helper and lease incarnation.
        // A cursor captured before cold materialization belongs to epoch zero.
        let helper_before = fixture.runtime.completion_cursor(&helper).await;
        report.release();
        let helper_after = fixture.runtime.wait_for_completion(&helper, helper_before, WAIT).await.unwrap();
        assert_ne!(helper_before, helper_after);
        let feedback = report.feedback.lock().unwrap().clone();
        assert_eq!(feedback.len(), 1, "actual result must reach the same helper model turn: {feedback:?}");
        assert!(!feedback[0].is_error, "{feedback:?}");
        let reply: serde_json::Value = serde_json::from_str(&feedback[0].text_content()).unwrap();
        assert_eq!(reply["operation"], source.operation.acquisition.id);
        assert_eq!(reply["phase"], "unavailable");
        let canonical = fixture.store.get(&source.operation.acquisition).await.unwrap().unwrap();
        assert_eq!(canonical.data.body["state"]["lifecycle_phase"], "unavailable");
        assert_eq!(canonical.data.body["helper_detail"]["report"], serde_json::json!({"kind":"unavailable","detail":DETAIL}));
        assert_eq!(canonical.data.body["helper_detail"]["helper"], serde_json::to_value(&source.helper.candidate().original_work).unwrap());
        assert_eq!(fixture.store.list_records(&acquisition_approved::scope(&fixture), None, 10).await.unwrap().len(), 1);
        let helper_history = acquisition_approved::consumed_native_input(
            runtime_store.as_ref(), &helper_session, &source.helper, None, deadline,
        ).await;
        assert_ne!(helper_history.state.input_id.to_string(), first_input_id);
        let a_history = acquisition_approved::native_history(runtime_store.as_ref(), &fixture.session).await;
        let actual_a: Vec<_> = a_history.iter().filter(|row| row.state.input_id.to_string() == first_input_id).collect();
        assert_eq!(actual_a.len(), 1);
        assert_eq!(actual_a[0].state.authority_contributors.len(), 1);
        assert_eq!(actual_a[0].state.authority_contributors[0].association(), &original);
        let final_source = fixture.store.get(&source_row.key).await.unwrap().unwrap();
        assert!(final_source == source_row, "report completion must not widen or replace helper source authority");
        let (_, planned, entered) = script.observations();
        assert!(planned > 0);
        assert_eq!(entered, 1, "one acquisition, no report-induced reacquisition");
    }))).catch_unwind().await;
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("configured helper report did not finish before the fixture deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

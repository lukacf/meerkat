//! Real native contributor ceilings and actual cold P custody. G is the existing
//! static fixture member; this does not qualify the shipping class customizer,
//! dynamic commissioning or MCP setup.
use super::idle_restore_tests::{close, finish, ids};
use super::*;
use crate::installed_host::operator_setup::{
    WorkflowHelperAuthorityRequest, WorkflowParentAuthoritySelection, workflow_operation_envelope,
};
use crate::private_originals::governed_submit::tests::first_input_tests::dedicated::DedicatedFixture;
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
use meerkat_toolkit::host_lifetime::policy::CompiledMemberClassInstance;

type Fixture = DedicatedFixture<meerkat::PersistentSessionService<meerkat::FactoryAgentBuilder>>;
fn selection(c: &Fixture) -> WorkflowParentAuthoritySelection {
    let native =
        c.f.lifetime()
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap();
    let member = native
        .preparation()
        .members
        .into_iter()
        .find(|member| member.member.member == "planning_lead")
        .unwrap();
    let instance = CompiledMemberClassInstance {
        class: id("operational-parent-class"),
        member,
    };
    let envelope = workflow_operation_envelope(native, &instance, 0, c.terms.deadline_ms).unwrap();
    WorkflowParentAuthoritySelection {
        commission_sha256: digest(b"exact selected future G commission"),
        instance,
        authority: WorkflowHelperAuthorityRequest {
            parent_issuance_id: EvidenceId::new("operational-parent-root").unwrap(),
            leaf_issuance_id: EvidenceId::new("operational-parent-leaf").unwrap(),
            envelope,
        },
    }
}
async fn prepare(c: &Fixture, selected: &WorkflowParentAuthoritySelection) {
    c.f.source
        .service_operator_setup(c.f.lifetime().clone())
        .unwrap()
        .configure_workflow_parent(&c.f.source, &c.parent.original, selected)
        .await
        .unwrap();
}
async fn parent_row(c: &Fixture) -> toolkit_store::StoredRecord {
    c.f.store
        .get(&c.f.source.key(&c.parent.original).unwrap())
        .await
        .unwrap()
        .unwrap()
}
fn revoke(c: &Fixture, selected: &WorkflowParentAuthoritySelection) {
    let native =
        c.f.lifetime()
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap();
    let grants = c.f.source.owner.grants();
    let leaf = grants
        .issued_reference(&selected.authority.leaf_issuance_id)
        .unwrap()
        .unwrap();
    let mut custody = c.f.machine.try_controller_grant_mutation().unwrap();
    grants
        .revoke(&native.grant_administrator, &leaf, &mut custody)
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_g_catalog_requires_independently_admitted_parent_operation_ceiling() {
    for operational in [false, true] {
        let c = Fixture::persistent_operational_parent().await;
        let body = AssertUnwindSafe(Box::pin(async {
            let selected = selection(&c);
            if operational {
                // Durable intent alone is not a grant or a native input.
                c.f.source
                    .select_workflow_parent_authority(&c.parent.original, selected.clone())
                    .await
                    .unwrap();
                let intent = parent_row(&c).await;
                let record =
                    c.f.source
                        .decode_record(&intent, &c.parent.original)
                        .unwrap();
                assert!(matches!(
                    c.f.source
                        .workflow_parent_basis(&record, &c.parent.original),
                    Err(PrivateOriginalError::Unavailable)
                ));
                assert!(
                    c.f.source
                        .owner
                        .grants()
                        .issued_reference(&selected.authority.parent_issuance_id)
                        .unwrap()
                        .is_none()
                );
                assert!(
                    c.f.source
                        .owner
                        .grants()
                        .issued_reference(&selected.authority.leaf_issuance_id)
                        .unwrap()
                        .is_none()
                );
                prepare(&c, &selected).await;
                let prepared = parent_row(&c).await;
                assert!(
                    c.f.source
                        .binding_evidence(&c.parent.original)
                        .await
                        .unwrap()
                        .is_none()
                );
                assert!(
                    ids(&c, &c.parent_session).await.is_empty(),
                    "selection cannot dispatch P"
                );
                prepare(&c, &selected).await;
                assert_eq!(
                    parent_row(&c).await,
                    prepared,
                    "same intent reuses native issuance IDs"
                );
                let mut changed = selected.clone();
                changed.commission_sha256 = digest(b"another commission");
                assert!(matches!(
                    c.f.source
                        .service_operator_setup(c.f.lifetime().clone())
                        .unwrap()
                        .configure_workflow_parent(&c.f.source, &c.parent.original, &changed)
                        .await,
                    Err(HostLifetimeError::Denied)
                ));
                assert_eq!(parent_row(&c).await, prepared);
                changed = selected.clone();
                changed.authority.leaf_issuance_id =
                    EvidenceId::new("foreign-parent-leaf").unwrap();
                assert!(matches!(
                    c.f.source
                        .service_operator_setup(c.f.lifetime().clone())
                        .unwrap()
                        .configure_workflow_parent(&c.f.source, &c.parent.original, &changed)
                        .await,
                    Err(HostLifetimeError::Denied)
                ));
                assert!(
                    c.f.source
                        .owner
                        .grants()
                        .issued_reference(&changed.authority.leaf_issuance_id)
                        .unwrap()
                        .is_none()
                );
                assert_eq!(parent_row(&c).await, prepared);
            } else {
                c.f.source
                    .prepare_workflow_parent_source(&c.parent.original)
                    .await
                    .unwrap();
            }
            let p =
                c.f.source
                    .submit_authenticated_original(c.parent.clone())
                    .await
                    .unwrap();
            finish(&c, "planning_parent", &p).await;
            let parent =
                c.f.source
                    .binding_evidence(&c.parent.original)
                    .await
                    .unwrap()
                    .unwrap();
            let parent_before = parent_row(&c).await;
            let parent_ids = ids(&c, &c.parent_session).await;
            assert_eq!(parent_ids.len(), 1);
            if operational {
                let lineage = parent
                    .candidate()
                    .authority_basis
                    .lineage_for(None)
                    .unwrap();
                assert_eq!(lineage.len(), 2);
                assert_eq!(lineage[0].grant_id, selected.authority.parent_issuance_id);
                assert_eq!(lineage[1].grant_id, selected.authority.leaf_issuance_id);
                c.f.source
                    .check_workflow_parent_commission(
                        &c.parent.original,
                        &selected.commission_sha256,
                    )
                    .await
                    .unwrap();
                assert!(matches!(
                    c.f.source
                        .check_workflow_parent_commission(
                            &c.parent.original,
                            &digest(b"different bound commission")
                        )
                        .await,
                    Err(PrivateOriginalError::Conflict)
                ));
                prepare(&c, &selected).await;
                assert_eq!(
                    parent_row(&c).await,
                    parent_before,
                    "bound exact replay changes no source bytes"
                );
            } else {
                assert!(crate::original_ingress::controller_only_basis(
                    &parent.candidate().authority_basis
                ));
                assert!(
                    matches!(
                        c.f.source
                            .service_operator_setup(c.f.lifetime().clone())
                            .unwrap()
                            .configure_workflow_parent(&c.f.source, &c.parent.original, &selected)
                            .await,
                        Err(HostLifetimeError::Denied)
                    ),
                    "bound controller-only P cannot be retrofitted"
                );
            }
            let probe = c.f.probe.as_ref().unwrap();
            assert_eq!(
                probe.captured().origin_session_id(),
                Some(&c.parent_session)
            );
            probe.arm_next_input();
            let a = c.f.submit().await;
            c.f.finish(&a).await.unwrap();
            let dispatch = probe.captured();
            assert_eq!(dispatch.origin_session_id(), Some(&c.f.session));
            let original = c.f.source.acquisition_original(&dispatch).await.unwrap();
            let planning =
                c.f.source
                    .prepare_workflow_planning(
                        &c.parent.original,
                        c.lead.clone(),
                        c.terms.clone(),
                        c.lineage.clone(),
                    )
                    .await
                    .unwrap();
            let (membership, snapshot) = join_waiter(
                &c.f,
                &planning,
                &original,
                &dispatch,
                &c.backend,
                &c.work,
                "parent-ceiling-waiter",
            )
            .await;
            let admitted = native(planning.launch(&snapshot).await.unwrap());
            finish(&c, "planning_lead", &admitted).await;
            let g =
                c.f.source
                    .binding_evidence(&c.lead.original)
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(
                g.candidate().contributing_work,
                vec![parent.candidate().original_work.clone()]
            );
            let feedback = c.catalog_probe.as_ref().unwrap().observed();
            assert_eq!(feedback.len(), 2);
            for name in ["reset-helper-search", "reset-helper-load"] {
                let result = &feedback[name];
                assert_eq!(
                    result.is_error,
                    !operational,
                    "{name}: {}",
                    result.text_content()
                );
                assert!(result.settlement_failures.is_empty());
                if operational {
                    let value: serde_json::Value =
                        serde_json::from_str(&result.text_content()).unwrap();
                    assert_eq!(
                        value["catalog_exact"], true,
                        "actual catalog owner answered"
                    );
                } else {
                    assert!(
                        result.text_content().contains("current authorization"),
                        "{}",
                        result.text_content()
                    );
                }
            }
            assert_eq!(parent_row(&c).await, parent_before);
            assert_eq!(ids(&c, &c.parent_session).await, parent_ids);
            assert_eq!(
                ids(&c, &c.lead_session).await.len(),
                1,
                "one G input, no resubmission"
            );
            let closed =
                c.f.source
                    .workflow_store()
                    .unwrap()
                    .depart_setup_workflow(&membership)
                    .await
                    .unwrap();
            assert_eq!(closed.active_waiters(), 0);
            assert!(closed.sealed());
        }))
        .catch_unwind()
        .await;
        close(&c, body).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operational_parent_cold_restore_checks_actual_grants_without_new_input() {
    for revoked in [false, true] {
        let c = Fixture::persistent_operational_parent().await;
        let selected = selection(&c);
        let mut saved = None;
        let body = AssertUnwindSafe(Box::pin(async {
            prepare(&c, &selected).await;
            let p =
                c.f.source
                    .submit_authenticated_original(c.parent.clone())
                    .await
                    .unwrap();
            finish(&c, "planning_parent", &p).await;
            let association =
                c.f.source
                    .binding_evidence(&c.parent.original)
                    .await
                    .unwrap()
                    .unwrap();
            let input_ids = ids(&c, &c.parent_session).await;
            assert_eq!(input_ids.len(), 1);
            if revoked {
                revoke(&c, &selected);
            }
            saved = Some((
                c.durable_state(),
                association,
                parent_row(&c).await,
                input_ids,
            ));
        }))
        .catch_unwind()
        .await;
        close(&c, body).await;
        drop(c);
        let (durable, association, row, input_ids) = saved.unwrap();
        let c = Fixture::reopen_operational_parent(durable).await;
        let body = AssertUnwindSafe(Box::pin(async {
            // Only P entered in this fixture. Join the actual bootstrap-started
            // custody worker before this direct idle reconstruction step, just
            // as the A/P/G restore fixture does, without changing its evidence.
            let before = parent_row(&c).await;
            assert_eq!(before, row);
            let record =
                c.f.source
                    .decode_record(&before, &c.parent.original)
                    .unwrap();
            assert!(
                record
                    .custody_attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.decision()
                        == crate::original_ingress::custody_attempt::CustodyDecision::Promoted)
            );
            c.f.source.stop_custody_recovery();
            tokio::time::timeout(GUARD, c.f.source.join_custody_recovery())
                .await
                .expect("join existing custody recovery before idle P restoration")
                .unwrap();
            assert_eq!(parent_row(&c).await, before);
            assert!(!c.f.source.owner.has_admitted_source(&association).unwrap());
            assert_eq!(c.f.model_requests(), 0);
            // Restore still refuses while the real source writer is owned.
            let writer = c.f.source.owner.source_writer().await;
            assert_eq!(
                c.f.source
                    .restore_workflow_parent_source(&c.parent.original)
                    .await,
                Err(PrivateOriginalError::Unavailable)
            );
            assert!(!c.f.source.owner.has_admitted_source(&association).unwrap());
            drop(writer);
            let observed =
                c.f.source
                    .restore_workflow_parent_source(&c.parent.original)
                    .await;
            if revoked {
                assert!(matches!(observed, Err(PrivateOriginalError::Unavailable)));
                assert!(!c.f.source.owner.has_admitted_source(&association).unwrap());
            } else {
                observed.unwrap();
                assert!(c.f.source.owner.has_admitted_source(&association).unwrap());
                assert_eq!(
                    c.f.source
                        .owner
                        .historical_task_custody_for_test(&association),
                    Some((true, true))
                );
                c.f.source
                    .restore_workflow_parent_source(&c.parent.original)
                    .await
                    .unwrap();
                assert_eq!(
                    c.f.source
                        .binding_evidence(&c.parent.original)
                        .await
                        .unwrap(),
                    Some(association)
                );
            }
            assert_eq!(parent_row(&c).await, row);
            assert_eq!(ids(&c, &c.parent_session).await, input_ids);
            assert!(ids(&c, &c.lead_session).await.is_empty());
            assert_eq!(
                c.f.model_requests(),
                0,
                "historical custody is not new admission or model replay"
            );
        }))
        .catch_unwind()
        .await;
        close(&c, body).await;
    }
}

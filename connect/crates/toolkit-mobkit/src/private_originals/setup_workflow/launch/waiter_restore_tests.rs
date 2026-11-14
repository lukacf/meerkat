//! Selected configured A persistence and actual idle ordinary resume. Restored
//! custody is not a new Find, workflow attachment or delegated G effect grant.
use super::idle_restore_tests::{assert_unattached_policy_closed, close, finish, ids};
use super::*;
use crate::private_originals::{
    governed_submit::tests::first_input_tests::dedicated::{DedicatedFixture, DurableDedicated},
    planning_source_evidence::PlanningSourceEvidence,
};
use meerkat_authorization_contracts::{
    constraints::DelegationDepth,
    work_association::{InputAuthorityAssociation, WorkAuthorityBasis},
};
use meerkat_core::{SessionServiceHistoryExt, lifecycle::InputId, service::SessionHistoryQuery};

pub(super) type PersistentDedicated =
    DedicatedFixture<meerkat::PersistentSessionService<meerkat::FactoryAgentBuilder>>;

pub(super) async fn histories(c: &PersistentDedicated) -> Vec<serde_json::Value> {
    let mut out = vec![];
    for session in [&c.f.session, &c.parent_session, &c.lead_session] {
        let rows =
            c.f.service
                .read_history(
                    session,
                    SessionHistoryQuery {
                        offset: 0,
                        limit: Some(100),
                    },
                )
                .await
                .unwrap();
        assert!(!rows.has_more);
        out.push(serde_json::to_value(rows.messages).unwrap());
    }
    out
}
pub(super) async fn all_ids(c: &PersistentDedicated) -> Vec<Vec<InputId>> {
    let mut out = vec![];
    for session in [&c.f.session, &c.parent_session, &c.lead_session] {
        out.push(ids(c, session).await);
    }
    out
}
/// Establish this component test's idle boundary with the actual already-started
/// custody worker. It is not acquisition-worker convergence: the fixture never
/// publishes gateway Ready and directly invokes one checked restoration step.
pub(super) async fn join_recovery_for_idle_restore(c: &PersistentDedicated) {
    use crate::original_ingress::custody_attempt::CustodyDecision;
    let source = &c.f.source;
    let mut settled = Vec::new();
    for reference in [
        c.f.original_reference(),
        &c.parent.original,
        &c.lead.original,
    ] {
        let row =
            c.f.store
                .get(&source.key(reference).unwrap())
                .await
                .unwrap()
                .unwrap();
        let record = source.decode_record(&row, reference).unwrap();
        assert!(
            record
                .custody_attempt
                .as_ref()
                .is_some_and(|attempt| attempt.decision() == CustodyDecision::Promoted),
            "idle reconstruction requires already-promoted canonical A/P/G attempts"
        );
        settled.push(row);
    }
    source.stop_custody_recovery();
    tokio::time::timeout(GUARD, source.join_custody_recovery())
        .await
        .expect("join the existing custody worker before this idle component step")
        .unwrap();
    for row in settled {
        assert_eq!(
            c.f.store.get(&row.key).await.unwrap().unwrap(),
            row,
            "joining idle recovery cannot manufacture or change canonical settlement"
        );
    }
}

pub(in crate::private_originals) struct Saved {
    pub(in crate::private_originals) durable: DurableDedicated,
    pub(in crate::private_originals) association: InputAuthorityAssociation,
    pub(in crate::private_originals) row: toolkit_store::StoredRecord,
    pub(in crate::private_originals) membership: SetupWorkflowWaiter,
    pub(in crate::private_originals) ids: Vec<Vec<InputId>>,
    pub(in crate::private_originals) histories: Vec<serde_json::Value>,
}
pub(super) async fn selected_then_close() -> Saved {
    selected_profile_then_close(false).await
}
pub(super) async fn selected_profile_then_close(shared: bool) -> Saved {
    let c = if shared {
        PersistentDedicated::persistent_effect_waiter().await
    } else {
        PersistentDedicated::persistent_configured_waiter().await
    };
    selected_fixture_then_close(c, shared).await
}
pub(in crate::private_originals) async fn selected_fixture_then_close(
    c: PersistentDedicated,
    shared: bool,
) -> Saved {
    let body = AssertUnwindSafe(Box::pin(async {
        let reference = c.f.original_reference();
        let key = c.f.source.source_record_key(reference).await.unwrap();
        let before = c.f.store.get(&key).await.unwrap().unwrap();
        if shared {
            c.member
                .prepare_effect_waiter_source(reference)
                .await
                .unwrap();
        } else {
            c.member.prepare_waiter_source(reference).await.unwrap();
        }
        let selected = c.f.store.get(&key).await.unwrap().unwrap();
        assert_ne!(selected.revision, before.revision);
        let record = c.f.source.decode_record(&selected, reference).unwrap();
        assert!(record.final_association.is_none() && record.bound_service_input.is_none());
        if shared {
            assert!(matches!(
                record.planning_source_evidence,
                Some(PlanningSourceEvidence::EffectWaiter { .. })
            ));
        } else {
            assert!(matches!(
                record.planning_source_evidence,
                Some(PlanningSourceEvidence::Waiter { .. })
            ));
        }
        if shared {
            c.member
                .prepare_effect_waiter_source(reference)
                .await
                .unwrap();
        } else {
            c.member.prepare_waiter_source(reference).await.unwrap();
        }
        assert_eq!(
            c.f.store.get(&key).await.unwrap().unwrap(),
            selected,
            "same prebind marker performs no write"
        );
        if !shared {
            assert_eq!(
                c.member.prepare_effect_waiter_source(reference).await,
                Err(PrivateOriginalError::Conflict)
            );
        }
        assert_eq!(
            c.f.store.get(&key).await.unwrap().unwrap(),
            selected,
            "Find-only evidence cannot acquire effect depth"
        );
        c.f.source
            .prepare_workflow_parent_source(&c.parent.original)
            .await
            .unwrap();
        let p = submit_planning_parent(&c.f.source, &c.parent).await;
        finish(&c, "planning_parent", &p).await;
        let probe = c.f.probe.as_ref().unwrap();
        assert_eq!(
            probe.captured().origin_session_id(),
            Some(&c.parent_session)
        );
        // The shared provider captured P. Its joined turn permits the existing
        // probe to capture the next actual input, A, without changing authority.
        probe.arm_next_input();
        let a = c.f.submit().await;
        c.f.finish(&a).await.unwrap();
        let dispatch = probe.captured();
        assert_eq!(dispatch.origin_session_id(), Some(&c.f.session));
        let original = c.f.source.acquisition_original(&dispatch).await.unwrap();
        assert_eq!(
            original.source_record.key,
            c.f.source.key(c.f.original_reference()).unwrap()
        );
        assert_eq!(
            original.original.association(),
            &c.f.binding().await.unwrap().unwrap()
        );
        let planning = prepare_planning(
            &c.f.source,
            &c.parent.original,
            &c.lead,
            &c.terms,
            &c.lineage,
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
            "selected-a-restore",
        )
        .await;
        let g = native(planning.launch(&snapshot).await.unwrap());
        finish(&c, "planning_lead", &g).await;
        let row = c.f.store.get(&key).await.unwrap().unwrap();
        let record = c.f.source.decode_record(&row, reference).unwrap();
        assert!(
            !c.f.source.stock_setup_source(&record, reference).unwrap(),
            "custom requester must exercise explicit retention"
        );
        let association = record.final_association.clone().unwrap();
        if !shared {
            assert_eq!(
                c.member.prepare_effect_waiter_source(reference).await,
                Err(PrivateOriginalError::Conflict)
            );
        }
        assert_eq!(
            c.f.store.get(&key).await.unwrap().unwrap(),
            row,
            "admitted Find-only A cannot adopt shared effects"
        );
        let input = record
            .bound_service_input
            .as_ref()
            .unwrap()
            .decode()
            .unwrap();
        let PrivateOriginalSubmission::Admitted { input_id, .. } = a.unwrap() else {
            panic!("actual A admission required")
        };
        assert_eq!(input.id().to_string(), input_id);
        tokio::time::timeout(GUARD, async {
            loop {
                let rows = c
                    .history
                    .as_ref()
                    .unwrap()
                    .load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(
                        &c.f.session,
                    ))
                    .await
                    .unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(&rows[0].state.input_id, input.id());
                meerkat_runtime::input_state::verify_exact_replay(&rows[0].state, &input).unwrap();
                if rows[0].state.persisted_input.is_none() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual A body retires before ordinary resume");
        Saved {
            durable: c.durable_state(),
            association,
            row,
            membership,
            ids: all_ids(&c).await,
            histories: histories(&c).await,
        }
    }))
    .catch_unwind()
    .await;
    let cleanup = AssertUnwindSafe(async {
        let result = c.close().await;
        assert!(result.is_ok(), "{result:?}");
    })
    .catch_unwind()
    .await;
    drop(c);
    match body {
        Err(panic) => std::panic::resume_unwind(panic),
        Ok(saved) => {
            if let Err(panic) = cleanup {
                std::panic::resume_unwind(panic);
            }
            saved
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selected_a_restores_actual_task_without_input_or_g_delegation() {
    let saved = selected_then_close().await;
    let c = PersistentDedicated::reopen(saved.durable).await;
    let body = AssertUnwindSafe(Box::pin(async {
        join_recovery_for_idle_restore(&c).await;
        assert_eq!(c.f.model_requests(), 0);
        assert!(
            !c.f.source
                .owner
                .has_admitted_source(&saved.association)
                .unwrap()
        );
        let grants_path = c.f.source_path.parent().unwrap().join("grants.jsonl");
        let grants_before = std::fs::read(&grants_path).unwrap();
        c.member
            .restore_waiter_source(c.f.original_reference())
            .await
            .unwrap();
        assert_eq!(
            c.f.cold_source_custody(&saved.association),
            Some((true, true))
        );
        c.member
            .restore_waiter_source(c.f.original_reference())
            .await
            .unwrap();
        let claims = saved.association.candidate();
        let WorkAuthorityBasis::GrantLineage { lineage } = &claims.authority_basis else {
            panic!("configured Find lineage")
        };
        let resolved =
            c.f.source
                .owner
                .grants()
                .resolve_lineage(
                    lineage,
                    &claims.logical_executor,
                    claims.represented_subject.as_ref(),
                )
                .unwrap();
        assert_eq!(
            resolved.restrictions().delegation_depth,
            DelegationDepth::remaining(0)
        );
        let g =
            c.f.source
                .binding_evidence(&c.lead.original)
                .await
                .unwrap()
                .unwrap();
        assert!(
            c.f.source
                .owner
                .grants()
                .resolve_lineage(
                    lineage,
                    &g.candidate().logical_executor,
                    claims.represented_subject.as_ref()
                )
                .is_err(),
            "A Find authority is not a G effect delegation"
        );
        assert!(
            !c.f.source.owner.has_admitted_source(&g).unwrap(),
            "A restore leaves G custody untouched"
        );
        let p =
            c.f.source
                .binding_evidence(&c.parent.original)
                .await
                .unwrap()
                .unwrap();
        assert!(
            !c.f.source.owner.has_admitted_source(&p).unwrap(),
            "A restore leaves P custody untouched"
        );
        assert_unattached_policy_closed(&c, &g);
        assert!(
            c.member.session().is_err(),
            "source restoration cannot complete a workflow attachment"
        );
        assert_eq!(
            std::fs::read(grants_path).unwrap(),
            grants_before,
            "no issuer or child was manufactured"
        );
        assert_eq!(
            c.f.store.get(&saved.row.key).await.unwrap().unwrap(),
            saved.row
        );
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selected_a_refuses_foreign_changed_missing_and_cancelled_evidence() {
    let saved = selected_then_close().await;
    let c = PersistentDedicated::reopen(saved.durable).await;
    let body = AssertUnwindSafe(Box::pin(async {
        let reference = c.f.original_reference();
        let mut foreign = reference.clone();
        foreign.event_sha256 = "f".repeat(64);
        assert_eq!(
            c.member.restore_waiter_source(&foreign).await,
            Err(PrivateOriginalError::Conflict)
        );
        let mut terms = c.terms.clone();
        terms.key = id("another-cohort");
        let other =
            c.f.source
                .workflow_store()
                .unwrap()
                .workflow_ref(&terms)
                .unwrap();
        assert_eq!(
            c.f.source
                .restore_workflow_waiter_source(&other, &c.lead.original, &c.terms.lead, reference)
                .await,
            Err(PrivateOriginalError::Unavailable)
        );
        for changed in [false, true] {
            let row = c.f.store.get(&saved.row.key).await.unwrap().unwrap();
            let mut data = saved.row.data.clone();
            if changed {
                data.body["bound_service_input"]["content"] = serde_json::json!("different A body");
            } else {
                data.body
                    .as_object_mut()
                    .unwrap()
                    .remove("bound_service_input");
            }
            c.f.store
                .compare_and_swap(toolkit_store::Transition {
                    key: row.key,
                    expected_revision: row.revision,
                    expected_digest: Some(row.content_digest),
                    claim: None,
                    data,
                    events: vec![],
                    effects: vec![],
                })
                .await
                .unwrap();
            assert_eq!(
                c.member.restore_waiter_source(reference).await,
                Err(if changed {
                    PrivateOriginalError::Conflict
                } else {
                    PrivateOriginalError::Unavailable
                })
            );
            assert!(c.f.cold_source_custody(&saved.association).is_none());
        }
        let row = c.f.store.get(&saved.row.key).await.unwrap().unwrap();
        let mut changed_basis = saved.row.data.clone();
        changed_basis.body["configured_setup"]["capability"]["parent"] =
            serde_json::to_value(&c.lineage[0]).unwrap();
        c.f.store
            .compare_and_swap(toolkit_store::Transition {
                key: row.key,
                expected_revision: row.revision,
                expected_digest: Some(row.content_digest),
                claim: None,
                data: changed_basis,
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        assert_eq!(
            c.member.restore_waiter_source(reference).await,
            Err(PrivateOriginalError::Conflict)
        );
        assert!(c.f.cold_source_custody(&saved.association).is_none());
        let row = c.f.store.get(&saved.row.key).await.unwrap().unwrap();
        c.f.store
            .compare_and_swap(toolkit_store::Transition {
                key: row.key,
                expected_revision: row.revision,
                expected_digest: Some(row.content_digest),
                claim: None,
                data: saved.row.data.clone(),
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        c.f.source
            .record_operation_cancelled(reference)
            .await
            .unwrap();
        assert_eq!(
            c.member.restore_waiter_source(reference).await,
            Err(PrivateOriginalError::Denied)
        );
        assert!(c.f.cold_source_custody(&saved.association).is_none());
        assert!(c.member.session().is_err());
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unselected_configured_a_keeps_body_with_application_and_cannot_retrofit() {
    let c = PersistentDedicated::persistent_configured_waiter().await;
    let body = AssertUnwindSafe(Box::pin(async {
        let a = c.f.submit().await;
        c.f.finish(&a).await.unwrap();
        let before = c.f.cold_source_record().await;
        let record =
            c.f.source
                .decode_record(&before, c.f.original_reference())
                .unwrap();
        assert!(record.configured_setup.is_some());
        assert!(
            !c.f.source
                .stock_setup_source(&record, c.f.original_reference())
                .unwrap()
        );
        assert!(record.planning_source_evidence.is_none() && record.bound_service_input.is_none());
        assert_eq!(
            c.member
                .prepare_waiter_source(c.f.original_reference())
                .await,
            Err(PrivateOriginalError::Unavailable)
        );
        assert_eq!(
            c.member
                .restore_waiter_source(c.f.original_reference())
                .await,
            Err(PrivateOriginalError::Unavailable)
        );
        assert_eq!(c.f.cold_source_record().await, before);
        assert_eq!(ids(&c, &c.f.session).await.len(), 1);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

//! Per-call diagnostic fidelity through the real persistent effect-waiter path.
//! The explicit setup-gate holder is test arrangement, not a reconstruction of
//! the competing holder or inner cause in the retained live-shared failure.
use super::idle_restore_tests::{close, finish};
use super::waiter_restore_tests::{PersistentDedicated, all_ids, histories};
use super::*;
use crate::admission_progress::{AdmissionProgress, AdmissionStage};
use crate::installed_host::acquisition::InstalledAcquisition;
use meerkat_authorization_contracts::work_association::GrantTarget;
use meerkat_toolkit::{host_lifetime::control::HostLifetimeError, setup::SetupError};

async fn effect_preparation_observation(contended: bool) {
    let c = PersistentDedicated::persistent_effect_waiter().await;
    // Retain the actual gate-owning thread until it has returned, including if
    // the bounded test body fails or is cancelled by its outer timeout.
    let mut holder = None;
    let body = AssertUnwindSafe(async {
        tokio::time::timeout(GUARD, async {
            c.member
                .prepare_effect_waiter_source(c.f.original_reference())
                .await
                .unwrap();
            c.f.source
                .prepare_workflow_parent_source(&c.parent.original)
                .await
                .unwrap();
            let parent =
                c.f.source
                    .submit_authenticated_original(c.parent.clone())
                    .await
                    .unwrap();
            finish(&c, "planning_parent", &parent).await;
            let probe = c.f.probe.as_ref().unwrap();
            probe.arm_next_input();
            let admitted = c.f.submit().await;
            c.f.finish(&admitted).await.unwrap();
            let dispatch = probe.captured();
            let original = c.f.source.acquisition_original(&dispatch).await.unwrap();
            assert!(
                original.reference == *c.f.original_reference(),
                "coordinator must retain the exact source reference"
            );
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
            // Preserve the actual canonical request/join and local interest,
            // but leave effect preparation to the same coordinator helper used
            // by production advance. Existing fixture users still prepare here.
            let (membership, _, acquisition) = join_waiter_context_with_effects(
                &c.f,
                &planning,
                &original,
                &dispatch,
                &c.backend,
                &c.work,
                "admission-refusal",
                None,
                None,
                None,
                false,
            )
            .await;
            let evidence = acquisition.request().evidence();
            let constraints = evidence.constraints();
            let tenant = id(&c.f.scope.realm);
            let deployment = id(&c.f.scope.owner);
            let route =
                c.f.source
                    .route(&planning.submission.original.route)
                    .unwrap();
            let context = SetupUseContext {
                tenant: &tenant,
                deployment: &deployment,
                realm: &route.realm,
                target: &original.original.association().candidate().target,
                policy_revision: &planning.terms.authority_revision,
                now_ms: u64::try_from(c.f.store.now_ms().await.unwrap()).unwrap(),
                authorization: dispatch.work_authorization().unwrap(),
            };
            assert_eq!(constraints.tenant, tenant);
            assert_eq!(constraints.deployment, deployment);
            let targets =
                ["restore-read-a", "restore-read-b"].map(|server| GrantTarget::McpServer {
                    server: server.into(),
                });
            let child_ids: Vec<_> = targets
                .iter()
                .map(|target| {
                    super::super::effects::child_id(
                        &c.lead.original,
                        &membership,
                        c.f.original_reference(),
                        target,
                    )
                    .unwrap()
                })
                .collect();
            for id in &child_ids {
                assert!(
                    c.f.source
                        .owner
                        .grants()
                        .issued_reference(id)
                        .unwrap()
                        .is_none()
                );
            }
            let grant_path = c.f.source_path.parent().unwrap().join("grants.jsonl");
            let grants_before = std::fs::read(&grant_path).unwrap();
            let inputs_before = all_ids(&c).await;
            let history_before = histories(&c).await;
            let requests_before = c.f.model_requests();
            let progress = AdmissionProgress::new();
            progress.bind_operation(&acquisition.key().id);

            if contended {
                let lifetime = c.f.lifetime().clone();
                let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
                let (release_tx, release_rx) = std::sync::mpsc::channel();
                let thread = std::thread::spawn(move || {
                    lifetime.with_current_setup(|| {
                        let _ = entered_tx.send(());
                        release_rx
                            .recv_timeout(GUARD)
                            .map_err(|_| HostLifetimeError::Unavailable)?;
                        Ok(())
                    })
                });
                holder = Some((release_tx, thread));
                entered_rx
                    .await
                    .expect("the real setup owner entered before preparation");
            }
            let result = InstalledAcquisition::prepare_workflow_effects(
                &planning,
                &membership,
                &original,
                SetupAccess {
                    evidence,
                    context: &context,
                    references: &Inline,
                },
                &progress,
            )
            .await;
            // Exercise the same returned-boundary fallback: it must not replace
            // the refusal retained before departure, even if cleanup ran last.
            if result.is_err() {
                progress.refused();
            }
            if let Some((release, thread)) = holder.take() {
                let _ = release.send(());
                assert!(thread.join().expect("actual setup holder joined").is_ok());
            }
            let observation = progress.observation();
            assert_eq!(observation.operation, Some(acquisition.key().id.as_str()));
            if contended {
                assert!(matches!(
                    result,
                    Err(SetupError::NativeExecutionUnavailable)
                ));
                assert_eq!(
                    observation.first_refusal,
                    Some("advance.workflow_prepare_effects")
                );
                assert_eq!(
                    observation.last_stage,
                    "advance.workflow_depart_after_failure"
                );
                assert_eq!(observation.cleanup, "completed");
                let after =
                    c.f.source
                        .workflow_store()
                        .unwrap()
                        .load_setup_workflow(membership.workflow())
                        .await
                        .unwrap();
                assert!(after.departed(&membership).unwrap());
                let interests = planning.interests.lock().unwrap();
                let interest = interests
                    .iter()
                    .find(|row| row.membership == membership)
                    .unwrap();
                assert!(interest.closed && interest.effects.is_none());
                drop(interests);
                for id in &child_ids {
                    assert!(
                        c.f.source
                            .owner
                            .grants()
                            .issued_reference(id)
                            .unwrap()
                            .is_none()
                    );
                }
                assert_eq!(
                    std::fs::read(&grant_path).unwrap(),
                    grants_before,
                    "refusal plus departure cannot invent an effect grant"
                );
            } else {
                result.expect("a fresh uncontended fixture reaches actual effect issuance");
                assert_eq!(observation.first_refusal, None);
                assert_eq!(observation.last_stage, "advance.workflow_prepare_effects");
                assert_eq!(observation.cleanup, "not_entered");
                for id in &child_ids {
                    assert!(
                        c.f.source
                            .owner
                            .grants()
                            .issued_reference(id)
                            .unwrap()
                            .is_some()
                    );
                }
                assert_ne!(std::fs::read(&grant_path).unwrap(), grants_before);
                assert!(
                    !c.f.source
                        .workflow_store()
                        .unwrap()
                        .load_setup_workflow(membership.workflow())
                        .await
                        .unwrap()
                        .departed(&membership)
                        .unwrap()
                );
                // The existing exact departure owner closes the successful
                // control's real grants before fixture shutdown.
                assert!(
                    planning
                        .depart_waiter(&membership)
                        .await
                        .unwrap()
                        .departed(&membership)
                        .unwrap()
                );
            }
            assert_eq!(
                all_ids(&c).await,
                inputs_before,
                "no new native input or G launch"
            );
            assert_eq!(histories(&c).await, history_before);
            assert_eq!(c.f.model_requests(), requests_before);
        })
        .await
        .expect("bounded actual workflow effect preparation control");
    })
    .catch_unwind()
    .await;
    // An assertion/timeout must not detach the setup gate holder or prevent
    // the fixture's original owner cleanup from running.
    let holder_result = holder.take().map(|(release, thread)| {
        let _ = release.send(());
        thread.join()
    });
    close(&c, body).await;
    if let Some(joined) = holder_result {
        assert!(
            joined
                .expect("actual setup holder joined during cleanup")
                .is_ok()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_effect_refusal_retains_first_boundary_after_exact_departure() {
    effect_preparation_observation(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_workflow_effect_preparation_has_no_refusal_observation() {
    effect_preparation_observation(false).await;
}

#[test]
fn returned_admission_observation_does_not_accept_arbitrary_operation_text() {
    let progress = AdmissionProgress::new();
    for value in ["request text", "setup-short", "setup-\n", "setup-PRIVATE"] {
        progress.bind_operation(value);
        assert_eq!(progress.observation().operation, None);
    }
    let first = format!("setup-{}", "a".repeat(64));
    let second = format!("setup-{}", "b".repeat(64));
    progress.bind_operation(&first);
    progress.bind_operation(&second);
    progress.enter(AdmissionStage::AdvanceLoad);
    progress.refused();
    progress.enter(AdmissionStage::DepartAfterFailure);
    progress.refused();
    let observation = progress.observation();
    assert_eq!(observation.operation, Some(first.as_str()));
    assert_eq!(observation.first_refusal, Some("advance.load"));
    assert_eq!(
        observation.last_stage,
        "advance.workflow_depart_after_failure"
    );
    assert_eq!(observation.cleanup, "not_entered");
}

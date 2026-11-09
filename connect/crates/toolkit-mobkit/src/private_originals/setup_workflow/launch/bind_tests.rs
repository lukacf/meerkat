//! A real pending G binding is withdrawn when its trusted binder refuses.
//! No native outcome is synthesized by this fixture.
use super::*;
use crate::private_originals::governed_submit::PendingOriginalAdmission;
use meerkat_core::{SessionId, lifecycle::InputId};
use meerkat_runtime::SessionServiceRuntimeExt;
use std::sync::atomic::AtomicUsize;

struct BoundBeforeRefusal {
    session: SessionId,
    input: InputId,
    pending: Weak<PendingOriginalAdmission>,
}

struct RefuseAfterActualBind {
    inner: Arc<WorkflowBinder>,
    observation: Arc<Mutex<Option<BoundBeforeRefusal>>>,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl GovernedInputBinder for RefuseAfterActualBind {
    async fn try_bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, GovernedBindError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let bound = self.inner.try_bind(context, input).await?;
        let held = self
            .inner
            .delegate
            .held
            .get()
            .expect("actual bound G source");
        let pending = self
            .inner
            .delegate
            .pending
            .get()
            .expect("actual pending G guard");
        let workflow = held
            .setup_workflow
            .get()
            .expect("actual pending G attachment");
        assert!(!workflow.accepted.load(Ordering::Acquire));
        assert!(workflow.current_source().is_err());
        assert!(held.unresolved.lock().unwrap().is_empty());
        assert!(
            bound.entry_fence.is_some(),
            "the real workflow binder supplied its fence"
        );
        assert!(
            bound.input.header().authority_association.as_ref()
                == Some(held.original.association()),
            "the actual pending input has the exact retained association"
        );
        let observed = BoundBeforeRefusal {
            session: context.session_id.clone(),
            input: bound.input.id().clone(),
            pending: Arc::downgrade(pending),
        };
        assert!(self.observation.lock().unwrap().replace(observed).is_none());
        // The actual pending guard is still retained by the real binder.
        // Meta receives a refusal, never this input, settlement or fence.
        drop(bound);
        Err(
            meerkat_mobkit::identity_first::GovernedBindRefusal::unavailable(
                "fixture refused after actual workflow binding",
            )
            .into(),
        )
    }

    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, detail: &str) {
        self.inner.retain_unresolved(settlement, detail);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_bind_refusal_withdraws_pending_source_and_replays_failed_launch() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        assert_eq!(
            f.inputs().await.unwrap(),
            2,
            "A and independent P are admitted"
        );
        let delivery = f
            .store
            .get_effect(&c.membership.delivery_effect().unwrap())
            .await
            .unwrap()
            .unwrap();
        let EffectClaimOutcome::Claimed(claimed) = f
            .store
            .claim_effect(
                &c.snapshot.reference().launch_effect(),
                c.snapshot.revision(),
                "post-bind-refusal",
                60_000,
            )
            .await
            .unwrap()
        else {
            panic!("first launch effect claimed")
        };
        let prepared = Arc::new(PreparedWorkflowLaunch {
            selection: c.planning.clone(),
            reference: c.snapshot.reference().clone(),
        });
        let route = f
            .source
            .route(&c.planning.submission.original.route)
            .unwrap();
        let observation = Arc::new(Mutex::new(None));
        let calls = Arc::new(AtomicUsize::new(0));
        let binder = Arc::new(RefuseAfterActualBind {
            inner: prepared.binder(&f.source, &route).await.unwrap(),
            observation: observation.clone(),
            calls: calls.clone(),
        });
        // Move the only strong binder handle to the actual native operation.
        // The test retains a Weak witness, not custody that could mask a leak.
        let refused = prepared
            .submit_with_binder(&f.source, binder, None)
            .await
            .unwrap();
        let observed = observation
            .lock()
            .unwrap()
            .take()
            .expect("actual binder ran");
        assert_eq!(calls.load(Ordering::Acquire), 1);
        assert_eq!(observed.session, f.session);
        tokio::time::timeout(GUARD, async {
            while observed.pending.upgrade().is_some() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("actual pending source owner released after Bind refusal");
        assert!(
            f.machine
                .input_state(&observed.session, &observed.input)
                .await
                .unwrap()
                .is_none(),
            "the exact bound G input never entered native history"
        );
        assert_eq!(f.inputs().await.unwrap(), 2);
        let held = f
            .source
            .retained
            .lock()
            .unwrap()
            .iter()
            .find(|held| held.reference == c.planning.submission.original)
            .cloned()
            .unwrap();
        let workflow = held.setup_workflow.get().unwrap();
        assert!(!workflow.accepted.load(Ordering::Acquire));
        assert!(workflow.current_source().is_err());
        assert!(held.unresolved.lock().unwrap().is_empty());
        assert!(
            c.planning
                .interests
                .lock()
                .unwrap()
                .iter()
                .all(|interest| interest.entries.is_empty()),
            "the Bind refusal cannot enter any launch source reservation"
        );
        assert!(matches!(refused, WorkflowLaunchObservation::NotAdmitted));
        c.planning
            .record_launch_observation(&f.source, claimed.lease.as_ref().unwrap(), &refused)
            .await
            .unwrap();
        let failed = f.store.get_effect(&claimed.key).await.unwrap().unwrap();
        assert_eq!(failed.status, EffectStatus::Failed);
        assert_eq!(failed.attempt, 1);
        assert!(failed.lease.is_none());
        // Definite nonadmission retains the durable association. A planned G
        // carries Lead evidence (prepare_workflow_planning selects it), so the
        // actual binder retained its exact first bound input as that
        // evidence's input before native refused it: the input and association
        // the binder observed above. The source acquires no service or setup
        // state.
        let bound_source = f
            .store
            .get(&c.planning.terms.planning_source)
            .await
            .unwrap()
            .unwrap();
        let source_record = f.source.decode_stored(&bound_source).unwrap();
        assert!(source_record.final_association.as_ref() == Some(held.original.association()));
        assert!(
            source_record.approved_task.is_none()
                && source_record.setup_request.is_none()
                && source_record.configured_setup.is_none()
        );
        assert!(matches!(
            &source_record.planning_source_evidence,
            Some(crate::private_originals::planning_source_evidence::PlanningSourceEvidence::Lead { .. })
        ));
        let retained = source_record
            .bound_service_input
            .as_ref()
            .expect("the Lead source retains its exact first bound input")
            .decode()
            .unwrap();
        assert_eq!(retained.id(), &observed.input);
        assert!(
            retained.header().authority_association.as_ref() == Some(held.original.association()),
            "the retained input is the refused first input with its exact association"
        );
        for _ in 0..2 {
            assert!(matches!(
                c.planning.launch(&c.snapshot).await.unwrap(),
                WorkflowLaunchObservation::NotAdmitted
            ));
            assert_eq!(
                f.store.get_effect(&failed.key).await.unwrap().unwrap(),
                failed
            );
            assert_eq!(
                f.store.get(&bound_source.key).await.unwrap().unwrap(),
                bound_source
            );
            assert_eq!(f.inputs().await.unwrap(), 2);
            assert_eq!(calls.load(Ordering::Acquire), 1);
        }
        assert_eq!(
            f.store
                .get_effect(&c.membership.delivery_effect().unwrap())
                .await
                .unwrap()
                .unwrap(),
            delivery,
            "launch refusal cannot acknowledge waiter disclosure"
        );
        // The same real ingress namespace can now admit a different source.
        // A stranded Candidate guard would make its actual binder refuse.
        let newer = event(&f, "after-definite-bind-refusal", vec![]).await;
        let accepted = f.source.submit_authenticated_original(newer).await.unwrap();
        assert_ne!(admitted_input(&f, &accepted), observed.input);
        f.finish(&Ok::<_, String>(accepted)).await.unwrap();
        assert_eq!(f.inputs().await.unwrap(), 3);
        assert!(
            f.machine
                .input_state(&observed.session, &observed.input)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            f.store.get_effect(&failed.key).await.unwrap().unwrap(),
            failed
        );
    }))
    .catch_unwind()
    .await;
    let cleanup = f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded Bind nonadmission control"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

// A workflow launch whose bind meets the custody recovery's control held by
// another actor (as a recovery round, a stop or an acceptance holds it) for a
// short while, released within Core's owner-change bound, waits for it as the
// shipped launch did and starts once: never NotAdmitted, never a Failed
// launch effect.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_whose_recovery_control_is_held_briefly_starts_once() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        assert_eq!(
            f.inputs().await.unwrap(),
            2,
            "A and independent P are admitted"
        );
        let EffectClaimOutcome::Claimed(claimed) = f
            .store
            .claim_effect(
                &c.snapshot.reference().launch_effect(),
                c.snapshot.revision(),
                "held-control-launch",
                60_000,
            )
            .await
            .unwrap()
        else {
            panic!("first launch effect claimed")
        };
        let prepared = Arc::new(PreparedWorkflowLaunch {
            selection: c.planning.clone(),
            reference: c.snapshot.reference().clone(),
        });
        let route = f
            .source
            .route(&c.planning.submission.original.route)
            .unwrap();
        // The launch bind meets the recovery control held for 300 ms just
        // before its recovery enrollment.
        {
            let source = f.source.clone();
            let enrollments = AtomicUsize::new(0);
            crate::source_hold_probe::install(Arc::new(move |site, _key| match site {
                crate::source_hold_probe::Site::OriginalEnroll
                    if enrollments.fetch_add(1, Ordering::SeqCst) == 0 =>
                {
                    crate::source_hold_probe::retain(
                        source
                            .custody_recovery
                            .hold_control_for_test(Duration::from_millis(300)),
                    );
                    None
                }
                _ => None,
            }));
        }
        let binder = prepared.binder(&f.source, &route).await.unwrap();
        let observed = prepared
            .submit_with_binder(&f.source, binder, None)
            .await
            .unwrap();
        tokio::task::spawn_blocking(crate::source_hold_probe::clear)
            .await
            .unwrap();
        assert!(
            matches!(
                &observed,
                WorkflowLaunchObservation::Native {
                    observation: PrivateOriginalSubmission::Admitted {
                        deduplicated: false,
                        ..
                    }
                }
            ),
            "the launch waited for the briefly held control and started once: {}",
            serde_json::to_string(&observed).unwrap()
        );
        c.planning
            .record_launch_observation(&f.source, claimed.lease.as_ref().unwrap(), &observed)
            .await
            .unwrap();
        let effect = f.store.get_effect(&claimed.key).await.unwrap().unwrap();
        assert_eq!(
            effect.status,
            EffectStatus::Succeeded,
            "the launch effect is not Failed"
        );
        assert_eq!(effect.attempt, 1);
        assert_eq!(
            f.inputs().await.unwrap(),
            3,
            "the launch input was admitted once"
        );
    }))
    .catch_unwind()
    .await;
    let cleanup = f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded held-control launch"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

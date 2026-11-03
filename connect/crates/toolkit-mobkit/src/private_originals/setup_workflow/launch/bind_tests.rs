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
    async fn bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, String> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let bound = self.inner.bind(context, input).await?;
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
        // Meta receives a Bind error, never this input, settlement or fence.
        drop(bound);
        Err("fixture refused after actual workflow binding".into())
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
            .submit_with_binder(&f.source, binder)
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
        // Definite nonadmission retains the durable association. This ordinary
        // planning source does not acquire service/setup cold-recovery state;
        // the actual input and pending guard were observed in the binder above.
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
        assert!(
            source_record.bound_service_input.is_none(),
            "ordinary planning source remains outside service/setup cold recovery"
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

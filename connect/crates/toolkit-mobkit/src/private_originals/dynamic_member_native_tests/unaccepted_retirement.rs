//! Unaccepted-resource cleanup shares Add's one-shot transfer and host owner.
use super::*;
use crate::host_gateway::{MemberClassCleanupObservation, MemberClassResourceCleanup};

pub(super) struct CleanupProbe {
    pub(super) observations: Mutex<Vec<&'static str>>,
    pub(super) released: AtomicUsize,
    pub(super) entered: tokio::sync::Notify,
    pub(super) release: tokio::sync::Semaphore,
}
impl CleanupProbe {
    pub(super) fn new(hold: bool) -> Arc<Self> {
        Arc::new(Self {
            observations: Mutex::new(Vec::new()),
            released: AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(usize::from(!hold)),
        })
    }
}
#[async_trait::async_trait]
impl MemberClassResourceCleanup for CleanupProbe {
    async fn cleanup(
        &self,
        observation: MemberClassCleanupObservation<'_>,
    ) -> Result<(), HostLifetimeError> {
        let release = match observation {
            MemberClassCleanupObservation::Preflight {
                cleanup: Ok(()), ..
            } => {
                self.observations.lock().unwrap().push("preflight");
                true
            }
            MemberClassCleanupObservation::UnacceptedRetirement(Ok(())) => {
                self.observations.lock().unwrap().push("unaccepted");
                true
            }
            MemberClassCleanupObservation::Settled(outcome) => {
                assert!(outcome.native.as_ref().is_ok_and(|result| {
                    result
                        .outcomes
                        .values()
                        .any(|outcome| matches!(outcome, RestoreOutcome::Created { .. }))
                }));
                self.observations.lock().unwrap().push("created");
                false
            }
            _ => panic!("unexpected cleanup observation"),
        };
        if release {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            self.released.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unaccepted_retirement_cannot_take_a_created_members_cleanup() {
    let fixture = Box::pin(Fixture::start()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 4,
        Box::pin(async {
            let admitted = fixture.admit(ADDED).await;
            let context = fixture.native.identity_first_context().unwrap();
            let started = fixture
                .gateway
                .add_member_class(
                    &context,
                    admitted.clone(),
                    spec(ADDED, &class().profile),
                    fixture.route(ADDED),
                    fixture.intent(ADDED),
                    tokio::time::Instant::now() + WAIT,
                    || async { Ok(()) },
                )
                .await
                .unwrap();
            let outcome = match started {
                MemberClassAddStart::Settled(outcome) => outcome,
                MemberClassAddStart::Unsettled(settlement) => settlement.settled().await.unwrap(),
            };
            assert!(
                outcome.authorization.is_ok() && outcome.cleanup.is_ok(),
                "{outcome:?}"
            );
            let native = outcome.native.unwrap();
            assert!(
                matches!(
                    native.outcomes.get(&AgentIdentity::parse(ADDED).unwrap()),
                    Some(RestoreOutcome::Created { .. })
                ),
                "{native:?}"
            );
            let before = fixture.policy.current_member_class(&admitted).unwrap();
            let cleanup = CleanupProbe::new(false);
            for handle in [admitted.clone(), fixture.admit(ADDED).await] {
                assert_eq!(
                    fixture
                        .gateway
                        .retire_unaccepted_member_class_with_cleanup(handle, cleanup.clone())
                        .await,
                    Err(HostLifetimeError::Denied)
                );
                assert_eq!(
                    fixture.policy.current_member_class(&admitted).unwrap(),
                    before,
                    "an accepted Add retains its exact authorization"
                );
            }
            assert!(
                cleanup.observations.lock().unwrap().is_empty(),
                "refusal never enters resource cleanup"
            );
            let status = fixture
                .native
                .identity_runtime()
                .unwrap()
                .status(&AgentIdentity::parse(ADDED).unwrap())
                .await
                .unwrap();
            assert_eq!(status.state, IdentityLifecycleState::Active);
            assert!(
                fixture
                    .source
                    .member_route(&fixture.route(ADDED).id)
                    .unwrap()
                    .is_some()
            );
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match checked {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("accepted member cancellation control timed out"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unaccepted_retirement_survives_observer_loss_and_joins_entered_write() {
    let captured = Arc::new(std::sync::OnceLock::new());
    let capture = captured.clone();
    let fixture = Box::pin(Fixture::start_with_policy_store(Some(Box::new(
        move |inner| {
            let store = Arc::new(HeldPublicationWrite::for_event(
                inner,
                "member-policy-retiring",
            ));
            assert!(capture.set(store.clone()).is_ok());
            store
        },
    ))))
    .await;
    let store = captured.get().unwrap().clone();
    let mut observer = None;
    let cleanup = CleanupProbe::new(true);
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 4,
        Box::pin(async {
            let admitted = fixture.admit("project-aborted").await;
            let replay = fixture.admit("project-aborted").await;
            let before = fixture.policy.current_member_class(&admitted).unwrap();
            assert_eq!(
                fixture.gateway.retire_unaccepted_member_class(replay).await,
                Err(HostLifetimeError::Denied),
                "a reconstructed handle owns no transfer right"
            );
            assert_eq!(
                fixture.policy.current_member_class(&admitted).unwrap(),
                before
            );

            let gateway = fixture.gateway.clone();
            let selected = admitted.clone();
            let resource = cleanup.clone();
            observer = Some(tokio::spawn(async move {
                gateway
                    .retire_unaccepted_member_class_with_cleanup(selected, resource)
                    .await
            }));
            store.entered.notified().await;
            assert_eq!(store.entered_count.load(Ordering::SeqCst), 1);
            assert_eq!(store.returned_count.load(Ordering::SeqCst), 0);
            let observation = observer.take().unwrap();
            observation.abort();
            assert!(observation.await.unwrap_err().is_cancelled());

            let refused = fixture
                .gateway
                .add_member_class(
                    &fixture.native.identity_first_context().unwrap(),
                    admitted.clone(),
                    spec("project-aborted", &class().profile),
                    fixture.route("project-aborted"),
                    fixture.intent("project-aborted"),
                    tokio::time::Instant::now() + WAIT,
                    || async { panic!("retirement owns the one-shot reservation") },
                )
                .await;
            assert!(
                matches!(
                    refused,
                    Err(MemberClassAddError::ReservationNotAccepted { .. })
                ),
                "a concurrent Add cannot take accepted retirement custody: {refused:?}"
            );
            fixture
                .gateway
                .begin_close(GatewayHostCloseReason::Shutdown)
                .unwrap();
            let close = fixture.gateway.join_close();
            tokio::pin!(close);
            assert!(
                tokio::time::timeout(Duration::from_millis(40), &mut close)
                    .await
                    .is_err(),
                "host close must join the actual entered write"
            );
            assert_eq!(store.returned_count.load(Ordering::SeqCst), 0);
            store.release.add_permits(1);
            cleanup.entered.notified().await;
            assert!(
                tokio::time::timeout(Duration::from_millis(40), &mut close)
                    .await
                    .is_err(),
                "the same host task joins resource cleanup after canonical retirement"
            );
            assert_eq!(cleanup.released.load(Ordering::SeqCst), 0);
            cleanup.release.add_permits(1);
            close.await.unwrap();
            assert_eq!(cleanup.released.load(Ordering::SeqCst), 1);
            assert_eq!(*cleanup.observations.lock().unwrap(), ["unaccepted"]);
            assert_eq!(store.returned_count.load(Ordering::SeqCst), 1);
            let observed = fixture
                .policy
                .observe_member_class(&instance("project-aborted").member.member)
                .await
                .unwrap();
            assert_eq!(observed.checkpoint.admission, *admitted.admission_id());
            assert_eq!(
                observed.admission.phase,
                crate::installed_host::dynamic_members::MemberClassAdmissionPhase::Retired
            );
            assert!(matches!(
                fixture.policy.current_member_class(&admitted),
                Err(HostLifetimeError::Denied)
            ));
            assert_eq!(
                fixture.native.mob_handle().list_members().await.len(),
                1,
                "reservation retirement never constructed a native member"
            );
            assert_eq!(fixture.factory.preparations.load(Ordering::SeqCst), 0);
            assert!(fixture.factory.calls.lock().unwrap().is_empty());
            assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 0);
        }),
    ))
    .catch_unwind()
    .await;
    store.release.add_permits(1);
    cleanup.release.add_permits(1);
    if let Some(observer) = observer {
        let _ = observer.await;
    }
    fixture.close().await;
    match checked {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("unaccepted retirement cleanup control timed out"),
    }
}

//! Dropping an Unsettled observer leaves actual native and cleanup custody
//! with the existing host operation. No native result is supplied by this test.
use super::*;
use crate::host_gateway::{MemberClassCleanupObservation, MemberClassResourceCleanup};
use meerkat_mobkit::identity_first::{AgentAddressability, IdentityRuntimeError};
use meerkat_toolkit::host_lifetime::policy::CompiledMemberClassPurpose;

#[derive(Debug)]
struct TerminalFacts {
    held_aborted: bool,
    authorization_refused: bool,
    canonical_cleanup_joined: bool,
    actual: String,
}

struct RefusedResourceCleanup {
    observed: Mutex<Option<TerminalFacts>>,
    calls: AtomicUsize,
    returned: AtomicUsize,
    released: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}

impl RefusedResourceCleanup {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            observed: Mutex::new(None),
            calls: AtomicUsize::new(0),
            returned: AtomicUsize::new(0),
            released: AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        })
    }
}

#[async_trait::async_trait]
impl MemberClassResourceCleanup for RefusedResourceCleanup {
    async fn cleanup(
        &self,
        observation: MemberClassCleanupObservation<'_>,
    ) -> Result<(), HostLifetimeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let facts = match observation {
            MemberClassCleanupObservation::Settled(outcome) => TerminalFacts {
                held_aborted: matches!(
                    &outcome.native,
                    Err(IdentityRuntimeError::HeldBootstrapAborted { .. })
                ),
                authorization_refused: outcome.authorization.is_err(),
                canonical_cleanup_joined: outcome.cleanup.is_ok(),
                actual: format!("{outcome:?}"),
            },
            _ => {
                self.entered.notify_one();
                return Err(HostLifetimeError::Denied);
            }
        };
        // Missing Created evidence is not absence evidence. Even this test
        // callback keeps unknown/Created outcomes rather than releasing them.
        let may_release =
            facts.held_aborted && facts.authorization_refused && facts.canonical_cleanup_joined;
        *self.observed.lock().unwrap() = Some(facts);
        self.entered.notify_one();
        if may_release {
            self.release.acquire().await.unwrap().forget();
            self.released.fetch_add(1, Ordering::SeqCst);
        }
        self.returned.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_unsettled_observer_still_joins_native_refusal_and_resource_cleanup() {
    let mut selected = class();
    selected.purpose = CompiledMemberClassPurpose::SharedWorkflow;
    let captured = Arc::new(std::sync::OnceLock::new());
    let capture = captured.clone();
    let fixture = Box::pin(Fixture::start_selected(
        selected,
        Some(Box::new(move |inner| {
            let store = Arc::new(HeldPublicationWrite::new(inner));
            assert!(capture.set(store.clone()).is_ok());
            store
        })),
    ))
    .await;
    let store = captured.get().unwrap().clone();
    let cleanup = RefusedResourceCleanup::new();
    let finalizers = Arc::new(AtomicUsize::new(0));
    let mut caller = None;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 5,
        Box::pin(async {
            fixture
                .gateway
                .bind_member_class_context(fixture.native.identity_first_context().unwrap())
                .unwrap();
            let parent = workflow::parent(&fixture).await;
            let (prepared, terms, _backend) =
                workflow::prepare(&fixture, &parent, "project-aborted").await;
            let admitted = fixture
                .gateway
                .reserve_workflow_member_class(
                    Id::new("unsettled-resource-admission").unwrap(),
                    &prepared,
                    &terms,
                )
                .await
                .unwrap();
            let model_calls = fixture.provider.0.load(Ordering::SeqCst);
            let gateway = fixture.gateway.clone();
            let retained = admitted.clone();
            let route = fixture.route("project-aborted");
            let intent = fixture.intent("project-aborted");
            let resource = cleanup.clone();
            let finalized = finalizers.clone();
            let mut internal = spec("project-aborted", &class().profile);
            internal.addressability = AgentAddressability::InternalOnly;
            caller = Some(tokio::spawn(async move {
                gateway
                    .add_workflow_member_class(
                        retained,
                        internal,
                        route,
                        intent,
                        tokio::time::Instant::now() + WAIT,
                        move || async move {
                            finalized.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        },
                        Some(resource),
                    )
                    .await
            }));
            store.entered.notified().await;
            assert_eq!(store.entered_count.load(Ordering::SeqCst), 1);
            assert_eq!(store.returned_count.load(Ordering::SeqCst), 0);
            let started = caller.as_mut().unwrap().await.unwrap().unwrap();
            let _joined = caller.take().unwrap();
            let MemberClassAddStart::Unsettled(settlement) = started else {
                panic!("actual native deadline must remain Unsettled while its entered write is held: {started:?}");
            };
            assert_eq!(store.returned_count.load(Ordering::SeqCst), 0);
            assert_eq!(cleanup.calls.load(Ordering::SeqCst), 0);
            assert_eq!(cleanup.released.load(Ordering::SeqCst), 0);
            assert!(fixture.source.member_route(&prepared.route().id).unwrap().is_none());
            assert!(fixture.gateway.completed_workflow_member_class(&admitted).is_err());
            // Losing observation does not cancel or settle the actual operation.
            drop(settlement);

            fixture.gateway.begin_close(GatewayHostCloseReason::Shutdown).unwrap();
            let close = fixture.gateway.join_close();
            tokio::pin!(close);
            assert!(tokio::time::timeout(Duration::from_millis(40), &mut close).await.is_err(),
                "the accepted operation still owns the entered SQLite write");
            assert_eq!(store.returned_count.load(Ordering::SeqCst), 0);
            assert_eq!(cleanup.calls.load(Ordering::SeqCst), 0);

            store.release.add_permits(1);
            cleanup.entered.notified().await;
            {
                let facts = cleanup.observed.lock().unwrap();
                let facts = facts.as_ref().expect("actual native settlement must reach cleanup");
                assert!(facts.held_aborted && facts.authorization_refused && facts.canonical_cleanup_joined,
                    "actual known native refusal and joined narrowing are required: {}", facts.actual);
            }
            assert_eq!(store.returned_count.load(Ordering::SeqCst), 1);
            let observed = fixture.policy.observe_member_class(&instance("project-aborted").member.member)
                .await.unwrap();
            assert_eq!(observed.checkpoint.admission, *admitted.admission_id());
            assert_eq!(observed.admission.phase,
                crate::installed_host::dynamic_members::MemberClassAdmissionPhase::Retired);
            assert!(matches!(fixture.policy.current_member_class(&admitted), Err(HostLifetimeError::Denied)));
            assert_eq!(fixture.native.mob_handle().list_members().await.len(), 1,
                "native refusal leaves only the original coordinator");
            let status = fixture.native.identity_runtime().unwrap()
                .status(&AgentIdentity::parse("project-aborted").unwrap()).await;
            assert!(!status.is_ok_and(|status| status.state == IdentityLifecycleState::Active));
            assert_eq!(finalizers.load(Ordering::SeqCst), 0,
                "an expired native attempt must not start the later finalizer");
            assert!(fixture.factory.calls.lock().unwrap().is_empty());
            assert_eq!(fixture.provider.0.load(Ordering::SeqCst), model_calls);

            assert!(tokio::time::timeout(Duration::from_millis(40), &mut close).await.is_err(),
                "host close joins entered resource cleanup even without the result receiver");
            assert_eq!(cleanup.calls.load(Ordering::SeqCst), 1);
            assert_eq!(cleanup.returned.load(Ordering::SeqCst), 0);
            assert_eq!(cleanup.released.load(Ordering::SeqCst), 0);
            cleanup.release.add_permits(1);
            close.await.unwrap();
            assert_eq!(cleanup.calls.load(Ordering::SeqCst), 1);
            assert_eq!(cleanup.returned.load(Ordering::SeqCst), 1);
            assert_eq!(cleanup.released.load(Ordering::SeqCst), 1);
        }),
    ))
    .catch_unwind()
    .await;
    // Always unblock entered native/resource work before the fixture's actual
    // native and host joins, including assertion failure and test timeout.
    store.release.add_permits(1);
    cleanup.release.add_permits(1);
    if let Some(caller) = caller {
        let _ = caller.await;
    }
    fixture.close().await;
    match checked {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("lost Unsettled resource cleanup control timed out"),
    }
}

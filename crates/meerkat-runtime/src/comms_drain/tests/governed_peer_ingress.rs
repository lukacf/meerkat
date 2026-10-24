//! The governed peer receive-admission seam: on a governed runtime, a peer
//! delivery whose sender the transport established is offered to the
//! installed `PeerIngressBinder` before admission, and admitted only as it
//! binds it.

use super::*;
use crate::input_admission_custody::{
    NativeInputAdmissionCompletion, NativeInputAdmissionCustody,
    NativeInputAdmissionSettlement as Settlement,
};
use crate::input_authority::tests::{TestIngress, bind_peer_delivery_for_test};
use crate::input_authority::{
    PeerIngressBindFuture, PeerIngressBindRefusal, PeerIngressBindRequest, PeerIngressBinder,
    PeerIngressBinding, PeerIngressClaimKind, PeerIngressOrigin,
};
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_core::PeerIngressSenderAuthentication as SenderAuthentication;
use meerkat_core::interaction::PeerIngressClaimTerminalOutcome as Terminal;

/// What the test binder answers one bind with.
#[derive(Clone, Copy, Debug)]
enum Answer {
    /// Bind under the requester the binder enrolled for this event key
    /// (`peer-requester` the first time): a verbatim re-bind.
    Bind,
    /// Bind under another requester than the one enrolled: a drifted re-bind.
    BindAs(&'static str),
    /// Bind naming another delivery's work.
    BindOtherWork,
    /// Bind, but change the input's content as well.
    BindAltered,
    Deny,
    NotReady,
    NotOffered,
    /// Wait until the test releases the bind, then bind.
    Stall,
}

/// One bind the test binder was asked for.
#[derive(Clone, Debug)]
struct BindCall {
    session_id: SessionId,
    origin: PeerIngressOrigin,
    event_key: IdempotencyKey,
    delivery_work: EvidenceId,
    input_id: meerkat_core::lifecycle::InputId,
}

/// The host's custody of what a binding enrolled: the requester bound per
/// event key, and every settlement native admission made of it. Shared by a
/// binder and its successor across a restart, as a host's durable custody is.
#[derive(Default)]
struct Custody {
    enrolled: std::sync::Mutex<HashMap<String, &'static str>>,
    settlements: std::sync::Mutex<Vec<Settlement>>,
}

struct RecordingCustody(Arc<Custody>);

impl NativeInputAdmissionCustody for RecordingCustody {
    fn settle(self: Box<Self>, settlement: Settlement) -> Option<NativeInputAdmissionCompletion> {
        self.0
            .settlements
            .lock()
            .expect("settlements")
            .push(settlement);
        None
    }
}

/// A peer ingress binder over [`TestIngress`]'s controller: it binds Core's
/// exact input under an association that host admits, as each session's
/// queued answers say (`Bind` once they run out).
struct TestBinder {
    selection: meerkat_core::ControllerModelSelection,
    custody: Arc<Custody>,
    answers: std::sync::Mutex<HashMap<SessionId, VecDeque<Answer>>>,
    calls: std::sync::Mutex<Vec<BindCall>>,
    stalled: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl TestBinder {
    fn new(selection: meerkat_core::ControllerModelSelection, custody: Arc<Custody>) -> Self {
        Self {
            selection,
            custody,
            answers: std::sync::Mutex::new(HashMap::new()),
            calls: std::sync::Mutex::new(Vec::new()),
            stalled: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        }
    }

    fn answer(&self, session_id: &SessionId, answers: impl IntoIterator<Item = Answer>) {
        self.answers
            .lock()
            .expect("answers")
            .entry(session_id.clone())
            .or_default()
            .extend(answers);
    }

    fn calls(&self) -> Vec<BindCall> {
        self.calls.lock().expect("calls").clone()
    }

    fn settlements(&self) -> Vec<Settlement> {
        self.custody
            .settlements
            .lock()
            .expect("settlements")
            .clone()
    }

    fn binding(
        &self,
        request: &PeerIngressBindRequest,
        requester: &'static str,
        work: EvidenceId,
    ) -> PeerIngressBinding {
        self.custody
            .enrolled
            .lock()
            .expect("enrolled")
            .entry(request.event_key.to_string())
            .or_insert(requester);
        PeerIngressBinding::new(bind_peer_delivery_for_test(
            request,
            requester,
            work,
            &self.selection,
        ))
        .with_admission_custody(Box::new(RecordingCustody(Arc::clone(&self.custody))))
    }

    fn enrolled_requester(&self, request: &PeerIngressBindRequest) -> &'static str {
        self.custody
            .enrolled
            .lock()
            .expect("enrolled")
            .get(&request.event_key.to_string())
            .copied()
            .unwrap_or("peer-requester")
    }
}

impl PeerIngressBinder for TestBinder {
    fn bind_peer_ingress(&self, request: PeerIngressBindRequest) -> PeerIngressBindFuture<'_> {
        Box::pin(async move {
            let answer = self
                .answers
                .lock()
                .expect("answers")
                .get_mut(&request.session_id)
                .and_then(VecDeque::pop_front)
                .unwrap_or(Answer::Bind);
            self.calls.lock().expect("calls").push(BindCall {
                session_id: request.session_id.clone(),
                origin: request.origin.clone(),
                event_key: request.event_key.clone(),
                delivery_work: request.delivery_work.clone(),
                input_id: request.input.id().clone(),
            });
            match answer {
                Answer::Bind => {
                    let requester = self.enrolled_requester(&request);
                    Ok(self.binding(&request, requester, request.delivery_work.clone()))
                }
                Answer::BindAs(requester) => {
                    Ok(self.binding(&request, requester, request.delivery_work.clone()))
                }
                Answer::BindOtherWork => {
                    let other = EvidenceId::new("peer-ingress-delivery:comms:another-delivery")
                        .expect("work id");
                    Ok(self.binding(&request, "peer-requester", other))
                }
                Answer::BindAltered => {
                    let binding =
                        self.binding(&request, "peer-requester", request.delivery_work.clone());
                    let (mut input, custody) = binding.into_parts();
                    if let crate::Input::Peer(peer) = &mut input {
                        peer.content = meerkat_core::types::ContentInput::Text(
                            "content the sender never sent".to_string(),
                        );
                    }
                    let binding = PeerIngressBinding::new(input);
                    Ok(match custody {
                        Some(custody) => binding.with_admission_custody(custody),
                        None => binding,
                    })
                }
                Answer::Deny => Err(PeerIngressBindRefusal::Denied),
                Answer::NotReady => Err(PeerIngressBindRefusal::NotReady(
                    meerkat_core::authorization::ControllerReadinessFailure::ExecutorUnavailable,
                )),
                Answer::NotOffered => Err(PeerIngressBindRefusal::NotOffered),
                Answer::Stall => {
                    self.stalled.notify_one();
                    self.release.notified().await;
                    let requester = self.enrolled_requester(&request);
                    Ok(self.binding(&request, requester, request.delivery_work.clone()))
                }
            }
        })
    }
}

/// A governed machine (with [`TestIngress`] installed) and its peer binder.
struct Governed {
    adapter: Arc<MeerkatMachine>,
    binder: Arc<TestBinder>,
}

impl Governed {
    /// A storeless governed machine; the binder installed when
    /// `install_binder`.
    fn storeless(install_binder: bool) -> Self {
        let machine = MeerkatMachine::ephemeral();
        let host = Arc::new(TestIngress::isolated(machine.generated_auth_lease_handle()));
        let selection = host.selection().clone();
        let machine = machine
            .with_native_work_authorization_host(host)
            .expect("governed host");
        let binder = Arc::new(TestBinder::new(selection, Arc::default()));
        if install_binder {
            machine
                .install_peer_ingress_binder(binder.clone())
                .expect("binder installs before any session");
        }
        Self {
            adapter: Arc::new(machine),
            binder,
        }
    }

    /// A persistent governed machine over `store`, under the controller
    /// `selection` (fresh when `None`), whose binder keeps `custody`.
    fn persistent(
        store: Arc<crate::store::InMemoryRuntimeStore>,
        selection: Option<meerkat_core::ControllerModelSelection>,
        custody: Arc<Custody>,
    ) -> Self {
        let mut chosen = None;
        let machine =
            MeerkatMachine::persistent_governed_with_host_for_test(store, None, |machine| {
                let host = Arc::new(match selection {
                    Some(selection) => TestIngress::with_selection(
                        machine.generated_auth_lease_handle(),
                        selection,
                    ),
                    None => TestIngress::isolated(machine.generated_auth_lease_handle()),
                });
                chosen = Some(host.selection().clone());
                host as Arc<dyn crate::input_authority::NativeWorkAuthorizationHost>
            })
            .expect("governed persistent machine");
        let binder = Arc::new(TestBinder::new(
            chosen.expect("the host's selection"),
            custody,
        ));
        machine
            .install_peer_ingress_binder(binder.clone())
            .expect("binder installs before any session");
        Self {
            adapter: Arc::new(machine),
            binder,
        }
    }

    async fn session(&self) -> SessionId {
        let session_id = SessionId::new();
        self.register(&session_id).await;
        session_id
    }

    async fn register(&self, session_id: &SessionId) {
        self.adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        self.adapter
            .support_governed_work_for_test(session_id)
            .await;
    }

    async fn input_count(&self, session_id: &SessionId) -> usize {
        self.adapter
            .meerkat_machine_spine_snapshot(session_id)
            .await
            .expect("registered session snapshot")
            .ledger
            .input_count
    }

    /// Drain `runtime`'s queue for `session_id` once, its items admitted by
    /// a transport that established their senders as `authentication`.
    async fn drain(
        &self,
        session_id: &SessionId,
        runtime: &Arc<OneShotPeerRequestRuntime>,
        authentication: SenderAuthentication,
    ) {
        runtime.queue.authenticate_senders(authentication);
        let drain = spawn_authorized_test_comms_drain(
            self.adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Duration::from_millis(10),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(10), drain)
            .await
            .expect("the drain exits")
            .expect("the drain task does not panic");
    }
}

fn message_runtime(candidate: PeerInputCandidate) -> Arc<OneShotPeerRequestRuntime> {
    Arc::new(OneShotPeerRequestRuntime::with_complete_authority(
        candidate,
        Arc::new(CountingPeerInteractionHandle::default()),
    ))
}

fn terminal_response() -> PeerInputCandidate {
    response_candidate(
        PeerInputClass::ResponseTerminal,
        Some(meerkat_core::interaction::TerminalityClass::Terminal {
            disposition: meerkat_core::interaction::TerminalDisposition::Completed,
        }),
    )
}

fn progress_response() -> PeerInputCandidate {
    response_candidate(
        PeerInputClass::ResponseProgress,
        Some(meerkat_core::interaction::TerminalityClass::Progress),
    )
}

/// A message, a request, a terminal response and a progress response from
/// a sender the listener verified are each offered to the binder with the
/// delivery's facts, admitted as bound (the input id is Core's one id for
/// the delivery), and the binding's custody settles `Admitted`. The durable
/// classes' claims end `RuntimeAccepted`; the responses are volatile.
#[tokio::test]
async fn a_verified_peer_delivery_is_admitted_as_the_binder_binds_it() {
    for (candidate, durable) in [
        (actionable_message_candidate(0), true),
        (
            inbound_peer_request_candidate(PeerInputClass::ActionableRequest),
            true,
        ),
        (terminal_response(), false),
        (progress_response(), false),
    ] {
        let fixture = Governed::storeless(true);
        let session_id = fixture.session().await;
        let class = candidate.class();
        let sender = candidate.from_peer_id().expect("a sender");
        let runtime = message_runtime(candidate);

        fixture
            .drain(
                &session_id,
                &runtime,
                SenderAuthentication::SignatureVerified,
            )
            .await;

        let calls = fixture.binder.calls();
        assert_eq!(calls.len(), 1, "{class:?}: the delivery is offered once");
        let call = &calls[0];
        assert_eq!(call.session_id, session_id);
        let PeerIngressOrigin::Comms(origin) = &call.origin else {
            panic!("{class:?}: a comms origin: {:?}", call.origin);
        };
        assert_eq!(origin.sender, sender, "{class:?}: the verified sender");
        assert_eq!(
            origin.sender_authentication,
            SenderAuthentication::SignatureVerified
        );
        assert_eq!(origin.class, class);
        assert_eq!(
            matches!(origin.claim, PeerIngressClaimKind::Durable { .. }),
            durable,
            "{class:?}: the claim kind"
        );
        assert_eq!(
            call.input_id,
            crate::input_authority::delivery_input_id(&call.origin, &call.event_key),
            "{class:?}: Core hands the binder its one id for the delivery"
        );
        assert_eq!(
            fixture.input_count(&session_id).await,
            1,
            "{class:?}: the bound delivery is admitted"
        );
        assert_eq!(
            fixture.binder.settlements(),
            vec![Settlement::Admitted],
            "{class:?}: native admission settles the binding's custody"
        );
        if durable {
            assert!(
                matches!(
                    runtime.queue.committed_outcomes().as_slice(),
                    [Some(Terminal::RuntimeAccepted { input_id })] if *input_id == call.input_id
                ),
                "{class:?}: {:?}",
                runtime.queue.committed_outcomes()
            );
        }
        assert_eq!(runtime.queue.remaining(), 0);
    }
}

/// The binder's `Denied` ends a durable delivery `RuntimeRejected`, admits
/// nothing, and leaves no custody to settle.
#[tokio::test]
async fn a_denied_binding_ends_the_delivery_rejected() {
    let fixture = Governed::storeless(true);
    let session_id = fixture.session().await;
    fixture.binder.answer(&session_id, [Answer::Deny]);
    let runtime = message_runtime(actionable_message_candidate(0));

    fixture
        .drain(
            &session_id,
            &runtime,
            SenderAuthentication::SignatureVerified,
        )
        .await;

    assert_eq!(fixture.binder.calls().len(), 1);
    assert_eq!(fixture.input_count(&session_id).await, 0);
    assert!(
        matches!(
            runtime.queue.committed_outcomes().as_slice(),
            [Some(Terminal::RuntimeRejected { .. })]
        ),
        "{:?}",
        runtime.queue.committed_outcomes()
    );
    assert!(fixture.binder.settlements().is_empty());
}

/// A malformed binding (Core's input with more changed than the
/// association and the ingress observation), or one naming another
/// delivery's work, is refused for good: the delivery ends
/// `RuntimeRejected`, its custody settles `NotAdmitted`, and the session's
/// next delivery is offered and admitted.
#[tokio::test]
async fn a_malformed_binding_ends_rejected_and_the_next_delivery_is_admitted() {
    for malformed in [Answer::BindAltered, Answer::BindOtherWork] {
        let fixture = Governed::storeless(true);
        let session_id = fixture.session().await;
        fixture
            .binder
            .answer(&session_id, [malformed, Answer::Bind]);
        let runtime = message_runtime(actionable_message_candidate(0));
        runtime.queue.push(actionable_message_candidate(1));

        fixture
            .drain(
                &session_id,
                &runtime,
                SenderAuthentication::SignatureVerified,
            )
            .await;

        assert_eq!(fixture.binder.calls().len(), 2, "{malformed:?}");
        assert!(
            matches!(
                runtime.queue.committed_outcomes().as_slice(),
                [
                    Some(Terminal::RuntimeRejected { .. }),
                    Some(Terminal::RuntimeAccepted { .. })
                ]
            ),
            "{malformed:?}: {:?}",
            runtime.queue.committed_outcomes()
        );
        assert_eq!(fixture.input_count(&session_id).await, 1, "{malformed:?}");
        assert_eq!(
            fixture.binder.settlements(),
            vec![Settlement::NotAdmitted, Settlement::Admitted],
            "{malformed:?}"
        );
    }
}

/// A `NotReady` binding admits nothing and keeps the durable delivery as
/// the session's FIFO head, unrefused: the later delivery behind it is not
/// offered. Once the binder binds, the head is admitted first and the later
/// delivery after it.
#[tokio::test]
async fn a_not_ready_binding_keeps_the_delivery_at_the_head_until_bound() {
    let fixture = Governed::storeless(true);
    let session_id = fixture.session().await;
    fixture.binder.answer(&session_id, [Answer::NotReady]);
    let head = actionable_message_candidate(0);
    let head_id = head.interaction.id;
    let runtime = message_runtime(head);
    runtime.queue.push(actionable_message_candidate(1));

    fixture
        .drain(
            &session_id,
            &runtime,
            SenderAuthentication::SignatureVerified,
        )
        .await;

    assert_eq!(fixture.binder.calls().len(), 1);
    assert!(runtime.queue.committed_outcomes().is_empty());
    assert_eq!(
        runtime.queue.remaining(),
        2,
        "the head is kept, not refused"
    );
    assert_eq!(fixture.input_count(&session_id).await, 0);

    fixture
        .drain(
            &session_id,
            &runtime,
            SenderAuthentication::SignatureVerified,
        )
        .await;

    let calls = fixture.binder.calls();
    assert_eq!(calls.len(), 3);
    let interaction = |call: &BindCall| match &call.origin {
        PeerIngressOrigin::Comms(origin) => origin.interaction_id,
        other => panic!("a comms origin: {other:?}"),
    };
    assert_eq!(interaction(&calls[0]), head_id);
    assert_eq!(
        interaction(&calls[1]),
        head_id,
        "the head is offered again first"
    );
    assert_ne!(interaction(&calls[2]), head_id);
    assert!(
        matches!(
            runtime.queue.committed_outcomes().as_slice(),
            [
                Some(Terminal::RuntimeAccepted { .. }),
                Some(Terminal::RuntimeAccepted { .. })
            ]
        ),
        "{:?}",
        runtime.queue.committed_outcomes()
    );
    assert_eq!(fixture.input_count(&session_id).await, 2);
}

/// A plain external event has no sender, and a delivery whose sender an
/// open-auth listener only claimed was not established: neither is offered
/// to the binder, and the governed runtime refuses them as before.
#[tokio::test]
async fn deliveries_without_an_established_sender_are_never_offered() {
    for (candidate, authentication) in [
        (
            host_event_candidate(InteractionId(Uuid::new_v4())),
            SenderAuthentication::SignatureVerified,
        ),
        (
            actionable_message_candidate(0),
            SenderAuthentication::Unverified,
        ),
    ] {
        let fixture = Governed::storeless(true);
        let session_id = fixture.session().await;
        let runtime = message_runtime(candidate);

        fixture.drain(&session_id, &runtime, authentication).await;

        assert!(
            fixture.binder.calls().is_empty(),
            "{authentication:?}: never offered"
        );
        assert_eq!(fixture.input_count(&session_id).await, 0);
        assert!(
            matches!(
                runtime.queue.committed_outcomes().as_slice(),
                [Some(Terminal::RuntimeRejected { .. })]
            ),
            "{authentication:?}: {:?}",
            runtime.queue.committed_outcomes()
        );
    }
}

/// Without a binder a governed runtime refuses a verified delivery as
/// before, and a binder that answers `NotOffered` leaves it to that same
/// refusal. An ungoverned runtime admits it unbound, and refuses a binder.
#[tokio::test]
async fn without_a_binding_a_governed_runtime_refuses_and_an_ungoverned_one_admits() {
    let unbound = Governed::storeless(false);
    let session_id = unbound.session().await;
    let runtime = message_runtime(actionable_message_candidate(0));
    unbound
        .drain(
            &session_id,
            &runtime,
            SenderAuthentication::SignatureVerified,
        )
        .await;
    assert!(unbound.binder.calls().is_empty());
    assert_eq!(unbound.input_count(&session_id).await, 0);
    assert!(matches!(
        runtime.queue.committed_outcomes().as_slice(),
        [Some(Terminal::RuntimeRejected { .. })]
    ));

    let not_offered = Governed::storeless(true);
    let session_id = not_offered.session().await;
    not_offered.binder.answer(&session_id, [Answer::NotOffered]);
    let runtime = message_runtime(actionable_message_candidate(0));
    not_offered
        .drain(
            &session_id,
            &runtime,
            SenderAuthentication::SignatureVerified,
        )
        .await;
    assert_eq!(not_offered.binder.calls().len(), 1);
    assert_eq!(not_offered.input_count(&session_id).await, 0);
    assert!(matches!(
        runtime.queue.committed_outcomes().as_slice(),
        [Some(Terminal::RuntimeRejected { .. })]
    ));

    let ungoverned = Arc::new(MeerkatMachine::ephemeral());
    let binder = Arc::new(TestBinder::new(
        TestIngress::isolated(ungoverned.generated_auth_lease_handle())
            .selection()
            .clone(),
        Arc::default(),
    ));
    assert_eq!(
        ungoverned.install_peer_ingress_binder(binder.clone()),
        Err(crate::PeerIngressBinderInstallRefused::Ungoverned)
    );
    let session_id = SessionId::new();
    ungoverned
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let runtime = message_runtime(actionable_message_candidate(0));
    runtime
        .queue
        .authenticate_senders(SenderAuthentication::SignatureVerified);
    let drain = spawn_authorized_test_comms_drain(
        ungoverned.clone(),
        session_id.clone(),
        runtime.clone(),
        Duration::from_millis(10),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(10), drain)
        .await
        .expect("the drain exits")
        .expect("the drain task does not panic");
    assert!(binder.calls().is_empty());
    assert!(matches!(
        runtime.queue.committed_outcomes().as_slice(),
        [Some(Terminal::RuntimeAccepted { .. })]
    ));
}

/// A redelivery bound verbatim deduplicates onto the first admission; one
/// bound under another requester is refused as a replay conflict, for good:
/// it ends `RuntimeRejected` and admits nothing more.
#[tokio::test]
async fn a_redelivery_deduplicates_and_a_drifted_rebind_is_refused() {
    let fixture = Governed::storeless(true);
    let session_id = fixture.session().await;
    let delivery = actionable_message_candidate(0);
    fixture.binder.answer(
        &session_id,
        [
            Answer::Bind,
            Answer::Bind,
            Answer::BindAs("another-requester"),
        ],
    );
    let runtime = message_runtime(delivery.clone());
    runtime.queue.push(delivery.clone());
    runtime.queue.push(delivery);

    fixture
        .drain(
            &session_id,
            &runtime,
            SenderAuthentication::SignatureVerified,
        )
        .await;

    assert_eq!(fixture.binder.calls().len(), 3);
    let outcomes = runtime.queue.committed_outcomes();
    assert!(
        matches!(
            outcomes.as_slice(),
            [
                Some(Terminal::RuntimeAccepted { .. }),
                Some(Terminal::RuntimeDeduplicated { .. }),
                Some(Terminal::RuntimeRejected { .. })
            ]
        ),
        "{outcomes:?}"
    );
    assert_eq!(fixture.input_count(&session_id).await, 1, "admitted once");
}

/// After a restart the host re-binds a delivery the store admitted, and
/// ended, with the association it enrolled: the redelivery deduplicates.
/// A re-bind naming another requester is refused as a replay conflict
/// although no live row holds the first admission any more: the store
/// still names its input under another qualified key.
#[tokio::test]
async fn after_a_restart_an_ended_delivery_deduplicates_and_a_drifted_rebind_is_refused() {
    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    let custody = Arc::<Custody>::default();
    let session_id = SessionId::new();
    let delivery = actionable_message_candidate(0);
    let (selection, admitted) = {
        let first = Governed::persistent(Arc::clone(&store), None, Arc::clone(&custody));
        first.register(&session_id).await;
        let runtime = message_runtime(delivery.clone());
        first
            .drain(
                &session_id,
                &runtime,
                SenderAuthentication::SignatureVerified,
            )
            .await;
        let outcomes = runtime.queue.committed_outcomes();
        let [Some(Terminal::RuntimeAccepted { input_id })] = outcomes.as_slice() else {
            panic!("the delivery is admitted: {outcomes:?}");
        };
        (first.binder.selection.clone(), input_id.clone())
    };
    // The admitted input ends, as its run's commit would end it.
    let runtime_id = MeerkatMachine::logical_runtime_id(&session_id);
    {
        use crate::store::RuntimeStore;
        let mut ended = store
            .load_input_state(&runtime_id, &admitted)
            .await
            .expect("read the admitted row")
            .expect("the admitted row");
        ended.seed.phase = crate::input_state::InputLifecycleState::Consumed;
        ended.seed.terminal_outcome = Some(crate::input_state::InputTerminalOutcome::Consumed);
        ended.seed.recovery_lane = None;
        ended.seed.last_run_id = Some(meerkat_core::lifecycle::RunId::new());
        ended.seed.last_boundary_sequence = Some(1);
        store
            .persist_input_state(
                &runtime_id,
                &crate::input_state::InputStatePersistenceRecord::from_machine_snapshot(ended)
                    .expect("an ended row"),
            )
            .await
            .expect("end the admitted input");
    }

    let restarted = Governed::persistent(Arc::clone(&store), Some(selection), custody);
    restarted.register(&session_id).await;
    restarted.binder.answer(
        &session_id,
        [Answer::Bind, Answer::BindAs("another-requester")],
    );
    let runtime = message_runtime(delivery.clone());
    runtime.queue.push(delivery);
    restarted
        .drain(
            &session_id,
            &runtime,
            SenderAuthentication::SignatureVerified,
        )
        .await;

    let outcomes = runtime.queue.committed_outcomes();
    assert!(
        matches!(
            outcomes.as_slice(),
            [
                Some(Terminal::RuntimeDeduplicated { .. }),
                Some(Terminal::RuntimeRejected { .. })
            ]
        ),
        "{outcomes:?}"
    );
    use crate::store::RuntimeStore;
    let row = store
        .load_input_state(&runtime_id, &admitted)
        .await
        .expect("read the admitted row")
        .expect("the admitted row");
    assert_eq!(
        row.seed.terminal_outcome,
        Some(crate::input_state::InputTerminalOutcome::Consumed),
        "the ended row is not written over"
    );
}

/// A binder that stalls for one session holds back only that session: the
/// drain awaits it holding no session or runtime lock (another admission
/// to the same session completes meanwhile), and another session's delivery
/// is admitted while it stalls. Released, the stalled delivery is admitted.
#[tokio::test]
async fn a_stalled_binder_holds_back_only_its_own_session() {
    let fixture = Governed::storeless(true);
    let stalled_session = fixture.session().await;
    let other_session = fixture.session().await;
    fixture.binder.answer(&stalled_session, [Answer::Stall]);
    let stalled_runtime = message_runtime(actionable_message_candidate(0));
    stalled_runtime
        .queue
        .authenticate_senders(SenderAuthentication::SignatureVerified);
    let stalled_drain = spawn_authorized_test_comms_drain(
        fixture.adapter.clone(),
        stalled_session.clone(),
        stalled_runtime.clone(),
        Duration::from_millis(10),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(10), fixture.binder.stalled.notified())
        .await
        .expect("the stalled session's bind is in flight");

    // Nothing of the stalled session is locked across the await.
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        fixture.adapter.accept_input_with_completion(
            &stalled_session,
            crate::Input::Prompt(crate::input::PromptInput::new("unbound", None)),
        ),
    )
    .await
    .expect("the stalled session admits other work meanwhile");
    assert!(refused.is_err(), "an unbound input is refused as before");
    tokio::time::timeout(
        Duration::from_secs(5),
        fixture
            .adapter
            .runtime_state(&MeerkatMachine::logical_runtime_id(&stalled_session)),
    )
    .await
    .expect("the stalled session's runtime answers meanwhile")
    .expect("runtime state");

    let other_runtime = message_runtime(actionable_message_candidate(1));
    fixture
        .drain(
            &other_session,
            &other_runtime,
            SenderAuthentication::SignatureVerified,
        )
        .await;
    assert_eq!(fixture.input_count(&other_session).await, 1);
    assert_eq!(fixture.input_count(&stalled_session).await, 0);

    fixture.binder.release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), stalled_drain)
        .await
        .expect("the released drain exits")
        .expect("the drain task does not panic");
    assert_eq!(fixture.input_count(&stalled_session).await, 1);
    assert!(matches!(
        stalled_runtime.queue.committed_outcomes().as_slice(),
        [Some(Terminal::RuntimeAccepted { .. })]
    ));
}

fn supervisor_delivery_input(session_id: &SessionId) -> crate::Input {
    peer_input_from_delivery_payload(
        session_id,
        PeerId::parse(PEER_ID_SUPERVISOR).expect("supervisor peer id"),
        BridgeDeliveryPayload {
            objective_id: None,
            system_prompt: None,
            injected_context: Vec::new(),
            transient_turn_context: None,
            supervisor: supervisor_bridge_spec(),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            input_id: Uuid::new_v4().to_string(),
            transcript_interaction_id: None,
            content: meerkat_core::types::ContentInput::Text("member work".to_string()),
            handling_mode: HandlingMode::Queue,
            expected_member: None,
            turn: None,
            outcome_tracking: None,
            bounded_result_spec: None,
        },
    )
    .expect("the delivery lowers")
}

/// A supervisor-bridge member delivery: a supervisor the transport did not
/// establish is never offered (it keeps the unbound path, which a governed
/// runtime refuses); an established one is offered as a supervisor origin
/// and its binding is admitted under the delivery's residency, its custody
/// settled `Admitted`; a binder that does not answer within the bridge's
/// bound leaves the delivery pending.
#[tokio::test]
async fn a_supervisor_delivery_is_bound_only_from_an_established_supervisor() {
    let fixture = Governed::storeless(true);
    let session_id = fixture.session().await;
    let supervisor = PeerId::parse(PEER_ID_SUPERVISOR).expect("supervisor peer id");

    let forged = bind_supervisor_member_delivery(
        &fixture.adapter,
        &session_id,
        SenderAuthentication::Unverified,
        supervisor,
        InteractionId(Uuid::new_v4()),
        false,
        supervisor_delivery_input(&session_id),
        None,
    )
    .await;
    assert!(matches!(forged, GovernedPeerBind::Unbound(_)));
    assert!(fixture.binder.calls().is_empty(), "never offered");

    for tracked in [false, true] {
        let input = supervisor_delivery_input(&session_id);
        let payload_input_id = input.id().clone();
        let GovernedPeerBind::Bound { input, custody } = bind_supervisor_member_delivery(
            &fixture.adapter,
            &session_id,
            SenderAuthentication::SignatureVerified,
            supervisor,
            InteractionId(Uuid::new_v4()),
            tracked,
            input,
            None,
        )
        .await
        else {
            panic!("tracked {tracked}: an established supervisor's delivery is bound");
        };
        let call = fixture.binder.calls().pop().expect("offered");
        assert!(
            matches!(
                &call.origin,
                PeerIngressOrigin::SupervisorDelivery(origin)
                    if origin.supervisor == supervisor && origin.tracked == tracked
            ),
            "{:?}",
            call.origin
        );
        assert_eq!(
            call.input_id, payload_input_id,
            "tracked {tracked}: a supervisor delivery keeps its payload input id"
        );
        let (outcome, _) = fixture
            .adapter
            .accept_governed_peer_input(
                &session_id,
                input,
                crate::meerkat_machine_types::MemberResidencyExpectation::PeerOnly,
                Some(custody),
            )
            .await
            .expect("the bound delivery is admitted");
        assert!(
            matches!(
                &outcome,
                crate::accept::AcceptOutcome::Accepted { input_id, .. }
                    if *input_id == payload_input_id
            ),
            "tracked {tracked}: the supervisor reads its own payload id back: {outcome:?}"
        );
    }
    assert_eq!(
        fixture.binder.settlements(),
        vec![Settlement::Admitted, Settlement::Admitted]
    );

    fixture.binder.answer(&session_id, [Answer::Stall]);
    let started = std::time::Instant::now();
    let pending = bind_supervisor_member_delivery(
        &fixture.adapter,
        &session_id,
        SenderAuthentication::SignatureVerified,
        supervisor,
        InteractionId(Uuid::new_v4()),
        false,
        supervisor_delivery_input(&session_id),
        None,
    )
    .await;
    assert!(matches!(pending, GovernedPeerBind::Pending(_)));
    assert!(started.elapsed() < Duration::from_secs(5), "bounded");
}

/// The binder is installed once, after the host, before any session: a
/// second install is refused typed and the first binder keeps serving; an
/// install once a session is registered is refused typed and the runtime
/// stays without a binder, so its deliveries take today's refusal.
#[tokio::test]
async fn a_binder_installs_once_and_only_before_any_session() {
    let fixture = Governed::storeless(true);
    let second = Arc::new(TestBinder::new(
        fixture.binder.selection.clone(),
        Arc::default(),
    ));
    assert_eq!(
        fixture.adapter.install_peer_ingress_binder(second.clone()),
        Err(crate::PeerIngressBinderInstallRefused::AlreadyInstalled)
    );
    let session_id = fixture.session().await;
    let runtime = message_runtime(actionable_message_candidate(0));
    fixture
        .drain(
            &session_id,
            &runtime,
            SenderAuthentication::SignatureVerified,
        )
        .await;
    assert_eq!(fixture.binder.calls().len(), 1, "the first binder serves");
    assert!(
        second.calls().is_empty(),
        "the refused binder is never asked"
    );
    assert!(matches!(
        runtime.queue.committed_outcomes().as_slice(),
        [Some(Terminal::RuntimeAccepted { .. })]
    ));

    let late = Governed::storeless(false);
    let session_id = late.session().await;
    assert_eq!(
        late.adapter
            .install_peer_ingress_binder(late.binder.clone()),
        Err(crate::PeerIngressBinderInstallRefused::SessionsRegistered)
    );
    assert!(!late.adapter.has_peer_ingress_binder());
    let runtime = message_runtime(actionable_message_candidate(0));
    late.drain(
        &session_id,
        &runtime,
        SenderAuthentication::SignatureVerified,
    )
    .await;
    assert!(late.binder.calls().is_empty(), "nothing was installed");
    assert!(matches!(
        runtime.queue.committed_outcomes().as_slice(),
        [Some(Terminal::RuntimeRejected { .. })]
    ));
}

/// A runtime with no binder treats every governed delivery exactly as a
/// binder answering `NotOffered`: Core admits Core's own projection unbound
/// (its id included, nothing Core derived for a binding), and a governed
/// runtime refuses it. A comms delivery ends `RuntimeRejected` either way, and
/// a supervisor delivery takes its unbound path with its input unchanged.
#[tokio::test]
async fn no_binder_is_exactly_the_not_offered_path() {
    let mut outcomes = Vec::new();
    for install in [false, true] {
        let fixture = Governed::storeless(install);
        let session_id = fixture.session().await;
        if install {
            fixture
                .binder
                .answer(&session_id, [Answer::NotOffered, Answer::NotOffered]);
        }
        let runtime = message_runtime(actionable_message_candidate(0));
        fixture
            .drain(
                &session_id,
                &runtime,
                SenderAuthentication::SignatureVerified,
            )
            .await;
        assert_eq!(
            fixture.input_count(&session_id).await,
            0,
            "install {install}"
        );
        assert!(fixture.binder.settlements().is_empty());
        let terminal = match runtime.queue.committed_outcomes().as_slice() {
            [Some(Terminal::RuntimeRejected { .. })] => "rejected",
            other => panic!("install {install}: {other:?}"),
        };

        let input = supervisor_delivery_input(&session_id);
        let projected = serde_json::to_vec(&input).expect("encode the projection");
        let GovernedPeerBind::Unbound(unbound) = bind_supervisor_member_delivery(
            &fixture.adapter,
            &session_id,
            SenderAuthentication::SignatureVerified,
            PeerId::parse(PEER_ID_SUPERVISOR).expect("supervisor peer id"),
            InteractionId(Uuid::new_v4()),
            false,
            input,
            None,
        )
        .await
        else {
            panic!("install {install}: the supervisor delivery is not bound");
        };
        assert_eq!(
            serde_json::to_vec(&unbound).expect("encode the unbound input"),
            projected,
            "install {install}: the unbound path admits Core's own projection"
        );
        outcomes.push((terminal, fixture.binder.calls().len()));
    }
    assert_eq!(outcomes, vec![("rejected", 0), ("rejected", 2)]);
}

/// A supervisor delivery the binder could not bind now is answered as a
/// rejected delivery typed `NotReady`, with the member runtime's state and the
/// cause in its reason, so the mob layer or the sender can tell it is
/// retryable with the same input id.
#[tokio::test]
async fn a_pending_supervisor_binding_is_rejected_typed_not_ready() {
    let fixture = Governed::storeless(true);
    let session_id = fixture.session().await;
    let response = governed_pending_delivery_rejection(
        &fixture.adapter,
        &session_id,
        "payload-input",
        &crate::RuntimeDriverError::ControllerReadinessUnavailable {
            reason: meerkat_core::authorization::ControllerReadinessFailure::ExecutorUnavailable,
        },
    )
    .await;
    assert_eq!(response.input_id, "payload-input");
    assert_eq!(response.canonical_input_id, None);
    let BridgeDeliveryOutcome::Rejected { cause, reason } = &response.outcome else {
        panic!("a rejected delivery: {response:?}");
    };
    assert!(
        matches!(cause, BridgeDeliveryRejectionCause::NotReady { .. }),
        "{cause:?}"
    );
    assert!(reason.contains("not ready"), "{reason}");
}

/// A governed supervisor delivery is admitted under its key's qualification
/// and keeps its payload input id as its input id. Its tracked read
/// (`tracked_delivery_input_state`) finds it by that id, live and from the
/// store after a restart, because the row's own retained association
/// qualifies the payload id to its admission key. Another id, and another
/// session, name nothing. The by-key reads keep only the machine binding, as
/// on every runtime: the payload id is no binding of a governed row.
#[tokio::test]
async fn a_governed_supervisor_delivery_is_read_by_its_payload_id() {
    use crate::service_ext::SessionServiceRuntimeExt as _;

    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    let custody = Arc::<Custody>::default();
    let session_id = SessionId::new();
    let supervisor = PeerId::parse(PEER_ID_SUPERVISOR).expect("supervisor peer id");
    let other_id = Uuid::new_v4().to_string();
    let (selection, payload_input_id) = {
        let first = Governed::persistent(Arc::clone(&store), None, Arc::clone(&custody));
        first.register(&session_id).await;
        let input = supervisor_delivery_input(&session_id);
        let payload_input_id = input.id().clone();
        let GovernedPeerBind::Bound { input, custody } = bind_supervisor_member_delivery(
            &first.adapter,
            &session_id,
            SenderAuthentication::SignatureVerified,
            supervisor,
            InteractionId(Uuid::new_v4()),
            true,
            input,
            None,
        )
        .await
        else {
            panic!("an established supervisor's delivery is bound");
        };
        first
            .adapter
            .accept_governed_peer_input(
                &session_id,
                input,
                crate::meerkat_machine_types::MemberResidencyExpectation::PeerOnly,
                Some(custody),
            )
            .await
            .expect("the bound delivery is admitted");
        let id = payload_input_id.to_string();
        let live = first
            .adapter
            .tracked_delivery_input_state(&session_id, &id)
            .await
            .expect("live tracked read")
            .expect("the payload id reads the live governed row");
        assert_eq!(live.state.input_id, payload_input_id);
        assert!(
            live.state
                .idempotency_key
                .as_ref()
                .is_some_and(|admitted| admitted.0 != id),
            "admitted under the key's qualification"
        );
        assert!(live.admitted_under_idempotency_key(&id));
        assert!(!live.admitted_under_idempotency_key(&other_id));
        assert!(
            first
                .adapter
                .tracked_delivery_input_state(&session_id, &other_id)
                .await
                .expect("live tracked read")
                .is_none(),
            "another id names nothing"
        );
        assert!(
            first
                .adapter
                .input_state_by_idempotency_key(&session_id, &id)
                .await
                .expect("live key read")
                .is_none(),
            "the payload id is no binding of the governed row"
        );
        assert!(matches!(
            first
                .adapter
                .tracked_delivery_input_state(&session_id, "not-an-input-id")
                .await,
            Err(crate::RuntimeDriverError::ValidationFailed { .. })
        ));
        (first.binder.selection.clone(), payload_input_id)
    };

    let restarted = Governed::persistent(Arc::clone(&store), Some(selection), custody);
    let id = payload_input_id.to_string();
    let durable = restarted
        .adapter
        .tracked_delivery_input_state(&session_id, &id)
        .await
        .expect("durable tracked read")
        .expect("the payload id reads the stored governed row");
    assert_eq!(durable.state.input_id, payload_input_id);
    assert!(durable.admitted_under_idempotency_key(&id));
    assert!(
        restarted
            .adapter
            .durable_input_state_by_idempotency_key(&session_id, &id)
            .await
            .expect("durable-only key read")
            .is_none(),
        "the payload id is no indexed key of the governed row"
    );
    assert!(
        restarted
            .adapter
            .tracked_delivery_input_state(&session_id, &other_id)
            .await
            .expect("durable tracked read")
            .is_none(),
        "another id names nothing"
    );
}

/// A row admitted without an association is admitted only under its own
/// key: never under the qualification a governed row's association would
/// give the same key.
#[test]
fn an_ungoverned_row_is_admitted_only_under_its_own_key() {
    let input_id = meerkat_core::lifecycle::InputId::new();
    let key = input_id.to_string();
    let mut stored = crate::input_state::StoredInputState::new_accepted(input_id.clone());
    stored.state.idempotency_key = Some(IdempotencyKey::new(key.clone()));
    assert!(!crate::input_authority::governed_row_admitted_under(
        &stored.state,
        &key
    ));
    assert!(stored.admitted_under_idempotency_key(&key), "its own key");
    stored.state.idempotency_key = Some(IdempotencyKey::new(format!(
        "local-governed-v1:{}",
        "00".repeat(32)
    )));
    assert!(!stored.admitted_under_idempotency_key(&key));
}

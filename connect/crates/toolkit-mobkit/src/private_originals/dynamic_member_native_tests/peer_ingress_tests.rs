//! Real Core comms ingress into actual Added receiver sessions. No constructed
//! binder request, imported sender permission, or synthetic admission proof.
use super::native_operations_tests::{native_input_states, ready};
use super::*;
use meerkat_authorization_contracts::audit::{
    AuditObservation, AuditTarget, StoredAuthorizationAuditObservation,
};
use meerkat_authorization_contracts::work_association::GrantTarget;
use meerkat_core::authorization::OperationObservedOutcome;
use meerkat_runtime::{Input, input_state::StoredInputState};

const SENDER: &str = "project-aborted";

// Application-selected topology over the actual desired roster, not Add grants.
// The initial coordinator-only roster has no eligible edge.
struct PairTopology(meerkat_mobkit::identity_first::ManagedPeerEdge);
#[async_trait::async_trait]
impl meerkat_mobkit::identity_first::TopologyProvider for PairTopology {
    async fn compute_edges(
        &self,
        identities: &[AgentIdentity],
        _context: &meerkat_mobkit::identity_first::TopologyContext,
    ) -> Result<
        Vec<meerkat_mobkit::identity_first::ManagedPeerEdge>,
        meerkat_mobkit::identity_first::TopologyError,
    > {
        Ok(
            if identities.contains(self.0.a()) && identities.contains(self.0.b()) {
                vec![self.0.clone()]
            } else {
                Vec::new()
            },
        )
    }
}

/// Observe real Core offers and the unchanged production binder result. This
/// fixture never constructs a request or overrides a binding/refusal.
pub(super) struct ObservedReceiver {
    source: Weak<PrivateOriginals>,
    offers: Mutex<Vec<(meerkat_core::SessionId, Input, bool)>>,
    declined: tokio::sync::Notify,
}
impl ObservedReceiver {
    pub(super) fn new(source: Arc<PrivateOriginals>) -> Self {
        Self {
            source: Arc::downgrade(&source),
            offers: Mutex::new(vec![]),
            declined: tokio::sync::Notify::new(),
        }
    }
}
impl meerkat_runtime::input_authority::PeerIngressBinder for ObservedReceiver {
    fn bind_peer_ingress(
        &self,
        request: meerkat_runtime::input_authority::PeerIngressBindRequest,
    ) -> meerkat_runtime::input_authority::PeerIngressBindFuture<'_> {
        Box::pin(async move {
            let session = request.session_id.clone();
            let input = request.input.clone();
            let source = self.source.upgrade().ok_or_else(|| {
                meerkat_runtime::input_authority::PeerIngressBindRefusal::Unavailable(
                    "fixture source closed".into(),
                )
            })?;
            let result = source.bind_peer_delivery(request).await;
            let not_ready = matches!(
                &result,
                Err(meerkat_runtime::input_authority::PeerIngressBindRefusal::NotReady(_))
            );
            let mut offers = self.offers.lock().unwrap();
            assert!(offers.len() < 128, "bounded actual peer offers");
            offers.push((session, input, not_ready));
            drop(offers);
            if not_ready {
                self.declined.notify_one();
            }
            result
        })
    }
}

fn receiver_class() -> CompiledMemberClass {
    let mut selected = class();
    for name in ["memory", "reply_to_peer", "send_message", "peers"] {
        selected.grants.push(CompiledMemberToolGrant {
            tool_name: name.into(),
            action: CompiledMemberToolAction::Invoke,
            consequence: if name == "peers" {
                CompiledToolConsequence::R0
            } else {
                CompiledToolConsequence::R1
            },
        });
    }
    selected
        .grants
        .sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
    selected
}

async fn start() -> Fixture {
    let fixture =
        Fixture::start_selected_source_with_recorder(receiver_class(), None, None, true).await;
    fixture
        .source
        .start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer())
        .unwrap();
    fixture
}

// Publish the desired spec through the same accepted Add finalizer used by
// the main durable fixture. Restore reads this actual roster publication.
async fn add(fixture: &Fixture, name: &str) -> meerkat_core::SessionId {
    let admitted = fixture.admit(name).await;
    let context = fixture.native.identity_first_context().unwrap();
    let desired = spec(name, &class().profile);
    let publish = desired.clone();
    let roster = fixture.roster.clone();
    let started = fixture
        .gateway
        .add_member_class(
            &context,
            admitted,
            desired,
            fixture.route(name),
            fixture.intent(name),
            tokio::time::Instant::now() + WAIT,
            move || async move {
                roster.upsert(publish);
                Ok(())
            },
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
    assert!(matches!(
        outcome
            .native
            .unwrap()
            .outcomes
            .get(&AgentIdentity::parse(name).unwrap()),
        Some(RestoreOutcome::Created { .. })
    ));
    fixture
        .native
        .identity_runtime()
        .unwrap()
        .status(&AgentIdentity::parse(name).unwrap())
        .await
        .unwrap()
        .session_id
        .unwrap()
}

// Addressable membership does not imply a trusted peer edge. Select this
// fixture's one reciprocal route through the real native wiring owner.
async fn wire_pair(fixture: &Fixture) {
    let handle = fixture.native.mob_handle();
    let receiver = handle
        .get_member(&meerkat_mobkit::member_comms_id::mob_member_id(ADDED))
        .await
        .unwrap()
        .unwrap();
    let sender = handle
        .get_member(&meerkat_mobkit::member_comms_id::mob_member_id(SENDER))
        .await
        .unwrap()
        .unwrap();
    assert!(
        receiver.wired_to.is_empty() && sender.wired_to.is_empty(),
        "Add alone must not grant a peer relationship"
    );
    handle
        .wire(receiver.agent_identity, sender.agent_identity)
        .await
        .unwrap();
    assert_pair_wired(fixture).await;
}

async fn assert_pair_wired(fixture: &Fixture) {
    let roster = fixture.native.mob_handle().roster().await;
    let receiver = roster
        .get_by_identity(&meerkat_mobkit::member_comms_id::mob_member_id(ADDED))
        .unwrap();
    let sender = roster
        .get_by_identity(&meerkat_mobkit::member_comms_id::mob_member_id(SENDER))
        .unwrap();
    assert_eq!(
        receiver.wired_to,
        std::collections::BTreeSet::from([sender.agent_identity.clone()])
    );
    assert_eq!(
        sender.wired_to,
        std::collections::BTreeSet::from([receiver.agent_identity.clone()])
    );
}

async fn peer(fixture: &Fixture, name: &str) -> meerkat_core::comms::PeerId {
    fixture
        .native
        .mob_handle()
        .get_member(&meerkat_mobkit::member_comms_id::mob_member_id(name))
        .await
        .unwrap()
        .unwrap()
        .peer_id()
        .unwrap()
}

async fn send(
    fixture: &Fixture,
    sender: &meerkat_core::SessionId,
    recipient: &meerkat_core::comms::PeerId,
) -> Result<meerkat_core::SendReceipt, meerkat_core::SendError> {
    // This trusted host call uses the actual sender session's comms owner.
    // It is only the incoming trigger; the receiver selects its own mandate.
    fixture
        .service
        .send_comms(
            sender,
            meerkat_core::CommsCommand::PeerMessage {
                to: meerkat_core::comms::PeerRoute::new(*recipient),
                body: "Please acknowledge and record this delivery".into(),
                blocks: None,
                content_taint: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                objective_id: None,
                in_reply_to: None,
            },
        )
        .await
        .unwrap()
}

async fn completed(
    fixture: &Fixture,
    session: &meerkat_core::SessionId,
    count: usize,
) -> StoredInputState {
    let rows = native_input_states(fixture, session).await;
    assert_eq!(
        rows.len(),
        count,
        "the actual durable comms receipt must precede this census"
    );
    let input = rows
        .iter()
        .max_by_key(|row| row.seed.admission_sequence)
        .unwrap()
        .state
        .input_id
        .clone();
    let receipt = tokio::time::timeout(
        WAIT,
        fixture.machine.wait_input_terminal_receipt(session, &input),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(
        receipt,
        Some(meerkat_runtime::terminal_status::InputTerminalReceiptWait::Resolved(_))
    ));
    native_input_states(fixture, session)
        .await
        .into_iter()
        .find(|row| row.state.input_id == input)
        .unwrap()
}

fn tool_succeeded(row: &StoredInputState, name: &str) {
    let audit: Vec<StoredAuthorizationAuditObservation> =
        serde_json::from_value(serde_json::to_value(row).unwrap()["authorization_audit"].clone())
            .unwrap();
    let operations = audit.iter().filter_map(|row| match &row.observation.observation {
        AuditObservation::Prepared { target, .. }
            if matches!(target.as_ref(), AuditTarget::Tool { tool_name, .. } if tool_name == name)
            => Some(row.observation.operation_id.clone()),
        _ => None,
    }).collect::<std::collections::HashSet<_>>();
    assert_eq!(
        operations.len(),
        1,
        "the actual selected native tool must prepare once"
    );
    assert!(
        audit
            .iter()
            .any(|row| operations.contains(&row.observation.operation_id)
                && matches!(
                    &row.observation.observation,
                    AuditObservation::Outcome {
                        outcome: OperationObservedOutcome::ToolDispatchReturned {
                            result_is_error: false,
                            terminal_error: None,
                            ..
                        }
                    }
                ))
    );
    assert!(
        audit
            .iter()
            .all(|row| !operations.contains(&row.observation.operation_id)
                || !matches!(
                    row.observation.observation,
                    AuditObservation::Refused { .. }
                        | AuditObservation::AuthorizationUnavailable { .. }
                ))
    );
}

async fn source_row(
    fixture: &Fixture,
    input: &StoredInputState,
) -> (StoredRecord, SourceRecord, PrivateOriginalRef) {
    let own = input
        .state
        .authority_contributors
        .iter()
        .filter(|own| own.input_id() == &input.state.input_id)
        .collect::<Vec<_>>();
    assert_eq!(own.len(), 1);
    let association = own[0].association();
    assert!(
        association.candidate().contributing_work.is_empty(),
        "the sender is never a contributor"
    );
    // Native completion and source promotion are distinct owners. Await the
    // actual recovery worker's canonical acknowledgement before asserting a
    // stable row or attempting an already-admitted replay.
    let row = tokio::time::timeout(WAIT, async {
        loop {
            let rows = fixture
                .source
                .store
                .list_records(&fixture.source.scope, None, 64)
                .await
                .unwrap();
            if let Some(row) = rows.into_iter().find(|row| {
                row.data.schema == SOURCE_SCHEMA
                    && fixture
                        .source
                        .decode_stored(row)
                        .unwrap()
                        .final_association
                        .as_ref()
                        == Some(association)
            }) {
                let record = fixture.source.decode_stored(&row).unwrap();
                if record.custody_attempt.as_ref().is_some_and(|attempt| {
                    attempt.decision()
                        == crate::original_ingress::custody_attempt::CustodyDecision::Promoted
                }) {
                    break row;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the native accepted delivery must reach actual source promotion");
    let decoded = fixture.source.decode_stored(&row).unwrap();
    assert!(
        decoded.peer_ingress.is_some()
            && decoded.class_input.is_none()
            && decoded.native_operations.is_none()
    );
    let reference = PrivateOriginalRef {
        route: decoded.event.route.clone(),
        original: decoded.event.original.clone(),
        event_sha256: digest(&canonical_json(&decoded.event).unwrap()),
    };
    assert_eq!(fixture.source.key(&reference).unwrap(), row.key);
    (row, decoded, reference)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn added_receiver_uses_own_delivery_grants_for_native_reply_and_recorder() {
    let edge = meerkat_mobkit::identity_first::ManagedPeerEdge::new(
        AgentIdentity::parse(ADDED).unwrap(),
        AgentIdentity::parse(SENDER).unwrap(),
    )
    .unwrap();
    let exclusive = CLASS_FIXTURE.lock().await;
    let fixture = Box::pin(Fixture::construct(
        receiver_class(),
        None,
        None,
        true,
        false,
        None,
        Some(Arc::new(PairTopology(edge.clone()))),
        exclusive,
    ))
    .await;
    fixture
        .source
        .start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer())
        .unwrap();
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 8, Box::pin(async {
        let receiver = add(&fixture, ADDED).await;
        let sender = add(&fixture, SENDER).await;
        ready(&fixture, &receiver).await;
        ready(&fixture, &sender).await;
        let receiver_peer = peer(&fixture, ADDED).await;
        let sender_peer = peer(&fixture, SENDER).await;
        // Fixture step only: the roster pass seats the coordinator Dormant,
        // and its peer identity is projected once its first wake builds it.
        // This step wakes it through the supported member wake (no input and
        // no turn) so the test can read that identity. It is not evidence for
        // the roster read projection of a Dormant seat, which Core owns.
        fixture
            .native
            .mob_handle()
            .ensure_member_live(&meerkat_mobkit::member_comms_id::mob_member_id(MEMBER))
            .await
            .unwrap();
        let coordinator_peer = peer(&fixture, MEMBER).await;
        let handle = fixture.native.mob_handle();
        for member in [ADDED, SENDER, MEMBER] {
            assert!(handle.get_member(&meerkat_mobkit::member_comms_id::mob_member_id(member))
                .await.unwrap().unwrap().wired_to.is_empty(), "Add must not refresh topology");
        }
        let before_refresh = send(&fixture, &sender, &receiver_peer).await;
        assert!(matches!(&before_refresh, Err(meerkat_core::SendError::PeerNotFound(id))
            if id == &receiver_peer.to_string()), "{before_refresh:?}");
        assert!(native_input_states(&fixture, &receiver).await.is_empty());
        assert!(native_input_states(&fixture, &sender).await.is_empty());
        // Both actual Add finalizers have published their specs. Refresh only
        // after they settle: the held callback owns this bootstrap controller.
        assert_eq!(fixture.roster.snapshot().len(), 3);
        let context = fixture.native.identity_first_context().unwrap();
        let refreshed = context.refresh_desired_topology_tracked().await.unwrap();
        assert_eq!(refreshed.managed_edges, vec![edge]);
        assert_pair_wired(&fixture).await;
        assert!(handle.get_member(&meerkat_mobkit::member_comms_id::mob_member_id(MEMBER))
            .await.unwrap().unwrap().wired_to.is_empty(), "the undeclared coordinator stays unwired");
        let undeclared = send(&fixture, &sender, &coordinator_peer).await;
        assert!(matches!(&undeclared, Err(meerkat_core::SendError::PeerNotFound(id))
            if id == &coordinator_peer.to_string()), "{undeclared:?}");
        assert!(native_input_states(&fixture, &receiver).await.is_empty());
        assert!(native_input_states(&fixture, &sender).await.is_empty());
        *fixture.factory.next_call.lock().unwrap() = Some("reply_to_peer");
        *fixture.factory.next_args.lock().unwrap() = serde_json::json!({"body":"Receiver-owned acknowledgement", "handling_mode":"queue"});
        let sent = send(&fixture, &sender, &receiver_peer).await.unwrap();
        assert!(matches!(sent, meerkat_core::SendReceipt::PeerMessageSent { delivery:
            meerkat_core::comms::PeerDeliveryOutcome::DurablyResolved { outcome:
                meerkat_core::interaction::PeerIngressTerminalOutcomeKind::Accepted }, .. }));
        let first = completed(&fixture, &receiver, 1).await;
        tool_succeeded(&first, "reply_to_peer");
        // The recipient's actual reply is independently admitted at the sender.
        // The tool's successful local dispatch alone is not this evidence.
        let reply = completed(&fixture, &sender, 1).await;
        let (_, reply_source, _) = source_row(&fixture, &reply).await;
        let (first_row, first_source, first_ref) = source_row(&fixture, &first).await;
        let first_binding = first_source.final_association.as_ref().unwrap();
        let basis = &first_binding.candidate().authority_basis;
        let WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } = basis else { panic!("delivery must have only exact targets"); };
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|row| row.target == GrantTarget::Peer { peer: sender_peer }));
        assert!(targets.iter().all(|row| !matches!(row.target, GrantTarget::PeerDirectory)));
        assert_ne!(reply_source.final_association.as_ref().unwrap().candidate().logical_executor,
            first_binding.candidate().logical_executor);
        let retained = cold_service_admission::decode_source_input(&first_source,
            first_source.bound_service_input.as_ref().unwrap()).unwrap();
        assert!(matches!(retained, Input::Peer(_)));
        // Actual native accepted input, including its retained own authority,
        // is the recovery witness. This invokes the pending Core Peer extension.
        meerkat_runtime::input_state::verify_exact_replay(&first.state, &retained).unwrap();
        let mut changed = serde_json::to_value(&retained).unwrap();
        changed["header"]["id"] = serde_json::to_value(meerkat_core::InputId::from_uuid(uuid::Uuid::new_v4())).unwrap();
        let changed: Input = serde_json::from_value(changed).unwrap();
        assert!(meerkat_runtime::input_state::verify_exact_replay(&first.state, &changed).is_err());

        *fixture.factory.next_call.lock().unwrap() = Some("memory");
        *fixture.factory.next_args.lock().unwrap() = serde_json::json!({"action":"remember","title":"Peer delivery", "body":"Receiver-owned standing Recorder write"});
        assert!(matches!(send(&fixture, &sender, &receiver_peer).await.unwrap(),
            meerkat_core::SendReceipt::PeerMessageSent { delivery: meerkat_core::comms::PeerDeliveryOutcome::DurablyResolved {
                outcome: meerkat_core::interaction::PeerIngressTerminalOutcomeKind::Accepted }, .. }));
        let second = completed(&fixture, &receiver, 2).await;
        tool_succeeded(&second, "memory");
        let (second_row, second_source, second_ref) = source_row(&fixture, &second).await;
        let second_binding = second_source.final_association.as_ref().unwrap();
        assert_ne!(first_binding.candidate().original_work, second_binding.candidate().original_work);
        for target in targets {
            assert_ne!(target.lineage[1], second_binding.candidate().authority_basis.lineage_for(Some(&target.target)).unwrap()[1]);
        }
        // Neither another delivery's association nor a new host Prompt can
        // reuse this delivery's children. The real source writer stays exact.
        let writer = fixture.source.owner.source_writer().await;
        assert!(fixture.source.persist_or_observe_binding(&second_ref, first_binding, true).await.is_err());
        drop(writer);
        assert_eq!(fixture.source.store.get(&second_row.key).await.unwrap(), Some(second_row));
        let count = native_input_states(&fixture, &receiver).await.len();
        let resubmit = fixture.source.submit_authenticated_original(PrivateOriginalSubmit {
            original: first_ref, content: "A later unrelated host prompt".into(),
            transport_authentication: first_source.event.original_authentication.clone(),
        }).await;
        assert!(!matches!(resubmit, Ok(PrivateOriginalSubmission::Admitted { .. })));
        assert_eq!(native_input_states(&fixture, &receiver).await.len(), count);
        assert_eq!(fixture.source.store.get(&first_row.key).await.unwrap(), Some(first_row));
        use meerkat_mobkit::identity_first::agent_memory::{AgentMemoryProvider, AgentMemorySelection, AgentMemoryRecallRequest};
        let memory = meerkat_mobkit::memory::SqliteAgentMemoryStore::open(fixture._directory.path().join("selected-recorder")).unwrap();
        let records = memory.recall(AgentMemoryRecallRequest {
            realm: fixture.lifetime.selected_policy().native_host.as_ref().unwrap().realm.to_string(),
            identity: AgentIdentity::parse(ADDED).unwrap(), query_text: None, query_terms: vec![],
            selection: AgentMemorySelection::Always, max_entries: 16,
        }).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].body, "Receiver-owned standing Recorder write");
        // The class exposes directory lookup, but this standing mandate does
        // not authorize it. Observe the actual native refusal on a new work.
        *fixture.factory.next_call.lock().unwrap() = Some("peers");
        *fixture.factory.next_args.lock().unwrap() = serde_json::json!({});
        send(&fixture, &sender, &receiver_peer).await.unwrap();
        let directory = completed(&fixture, &receiver, 3).await;
        let audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(
            serde_json::to_value(directory).unwrap()["authorization_audit"].clone()).unwrap();
        assert!(audit.iter().any(|row| matches!(&row.observation.observation,
            AuditObservation::Refused { target, reason: meerkat_core::authorization::OperationRefusalKind::Denied, .. }
                if matches!(target.as_ref(), AuditTarget::Tool { tool_name, .. } if tool_name == "peers"))));
        assert_eq!(native_input_states(&fixture, &sender).await.len(), 1);
        assert!(fixture.factory.calls.lock().unwrap().is_empty(), "native tools are not application callbacks");
    }))).catch_unwind().await;
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("receiver control did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_receiver_parent_refuses_actual_peer_delivery_without_input() {
    let fixture = Box::pin(start()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 8,
        Box::pin(async {
            let receiver = add(&fixture, ADDED).await;
            let sender = add(&fixture, SENDER).await;
            ready(&fixture, &receiver).await;
            ready(&fixture, &sender).await;
            wire_pair(&fixture).await;
            let recipient = peer(&fixture, ADDED).await;
            let admission = fixture
                .policy
                .member_class_admission(&instance(ADDED).member.member)
                .unwrap()
                .unwrap();
            let roots = admission
                .standing_operations
                .as_ref()
                .unwrap()
                .issued
                .as_ref()
                .unwrap();
            let context = fixture.source.callback_grant_context().unwrap();
            let mut mutation = fixture.machine.try_controller_grant_mutation().unwrap();
            context
                .grants
                .revoke(&context.operator, &roots.peer_message, &mut mutation)
                .unwrap();
            drop(mutation);
            let before = fixture
                .source
                .store
                .list_records(&fixture.source.scope, None, 64)
                .await
                .unwrap();
            let calls = fixture.provider.0.load(Ordering::SeqCst);
            let refused = send(&fixture, &sender, &recipient).await;
            assert!(
                matches!(
                    &refused,
                    Err(meerkat_core::SendError::DurableAdmissionRejected { .. })
                ),
                "the wired receiver must refuse admission through its revoked mandate: {refused:?}"
            );
            assert!(native_input_states(&fixture, &receiver).await.is_empty());
            assert!(native_input_states(&fixture, &sender).await.is_empty());
            assert_eq!(fixture.provider.0.load(Ordering::SeqCst), calls);
            let after = fixture
                .source
                .store
                .list_records(&fixture.source.scope, None, 64)
                .await
                .unwrap();
            let new = after
                .iter()
                .filter(|row| !before.iter().any(|prior| prior.key == row.key))
                .collect::<Vec<_>>();
            assert_eq!(
                new.len(),
                1,
                "the refused exact delivery retains one canonical terminal verdict"
            );
            let refused = fixture.source.decode_stored(new[0]).unwrap();
            assert!(refused.peer_ingress.is_some() && refused.operation_cancelled);
            assert!(
                refused.final_association.is_none()
                    && refused.bound_service_input.is_none()
                    && refused.custody_attempt.is_none()
            );
            for root in [&roots.peer_message, &roots.recorder_write] {
                assert_eq!(
                    context
                        .grants
                        .issued_reference(&root.grant_id)
                        .unwrap()
                        .as_ref(),
                    Some(root)
                );
            }
            assert!(matches!(
                context.grants.resolve_lineage(
                    std::slice::from_ref(&roots.peer_message),
                    &context.application,
                    admission
                        .route
                        .as_ref()
                        .unwrap()
                        .represented_subject
                        .as_ref()
                ),
                Err(meerkat_authorization::grants::GrantRefusal::Denied)
            ));
            assert_eq!(
                fixture
                    .policy
                    .member_class_admission(&instance(ADDED).member.member)
                    .unwrap()
                    .unwrap(),
                admission
            );
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("receiver refusal did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_peer_retries_after_actual_held_turn_without_replacing_source_custody() {
    let fixture = Box::pin(start()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 8, Box::pin(async {
        let receiver = add(&fixture, ADDED).await;
        let sender = add(&fixture, SENDER).await;
        ready(&fixture, &receiver).await;
        ready(&fixture, &sender).await;
        wire_pair(&fixture).await;
        let recipient = peer(&fixture, ADDED).await;
        *fixture.factory.next_call.lock().unwrap() = Some("memory");
        *fixture.factory.next_args.lock().unwrap() = serde_json::json!({"action":"remember",
            "title":"Queued peer", "body":"The retried delivery writes once"});
        let boundary = fixture.service.acquire_live_session_actor_turn_boundary_lease(&receiver).await.unwrap();
        let sending = send(&fixture, &sender, &recipient);
        tokio::pin!(sending);
        tokio::select! {
            _ = fixture.peer_observer.declined.notified() => (),
            outcome = &mut sending => panic!("held receiver must remain queued: {outcome:?}"),
        }
        let first_offer = fixture.peer_observer.offers.lock().unwrap().iter()
            .find(|(session, _, not_ready)| session == &receiver && *not_ready).unwrap().1.clone();
        let rows = fixture.source.store.list_records(&fixture.source.scope, None, 64).await.unwrap();
        let selected = rows.iter().filter(|row| row.data.schema == SOURCE_SCHEMA)
            .filter_map(|row| {
                let source = fixture.source.decode_stored(row).unwrap();
                source.peer_ingress.as_ref().filter(|peer| peer.check_input(&first_offer, false).is_ok())?;
                Some((row.clone(), source))
            }).collect::<Vec<_>>();
        assert_eq!(selected.len(), 1);
        let (retained_row, before) = &selected[0];
        assert!(before.final_association.is_none() && before.bound_service_input.is_none()
            && before.custody_attempt.is_none() && !before.operation_cancelled);
        assert!(native_input_states(&fixture, &receiver).await.is_empty());
        drop(boundary);
        assert!(matches!(sending.await.unwrap(), meerkat_core::SendReceipt::PeerMessageSent { delivery:
            meerkat_core::comms::PeerDeliveryOutcome::DurablyResolved { outcome:
                meerkat_core::interaction::PeerIngressTerminalOutcomeKind::Accepted }, .. }));
        let admitted = completed(&fixture, &receiver, 1).await;
        tool_succeeded(&admitted, "memory");
        let (after_row, after, reference) = source_row(&fixture, &admitted).await;
        assert_eq!(after_row.key, retained_row.key);
        assert!(after.peer_ingress.as_ref() == before.peer_ingress.as_ref());
        assert!(after.event == before.event);
        let last_offer = {
            let offers = fixture.peer_observer.offers.lock().unwrap();
            let last_offer = offers.iter().rev().find(|(session, _, not_ready)|
                session == &receiver && !not_ready).unwrap().1.clone();
            assert_eq!(last_offer.id(), first_offer.id());
            assert_ne!(last_offer.header().timestamp, first_offer.header().timestamp);
            after.peer_ingress.as_ref().unwrap().check_input(&last_offer, false).unwrap();
            last_offer
        };
        let exact = cold_service_admission::decode_source_input(&after, after.bound_service_input.as_ref().unwrap()).unwrap();
        meerkat_runtime::input_state::verify_exact_replay(&admitted.state, &exact).unwrap();
        // Replay the actual admitted delivery through the existing original
        // binder and native admission owner with a later projection time.
        // This is native replay, not a second physical transport redelivery.
        use meerkat_mobkit::identity_first::{GovernedBindContext, GovernedInputBinder};
        let mut offered = serde_json::to_value(&last_offer).unwrap();
        offered["header"]["timestamp"] = serde_json::to_value(
            last_offer.header().timestamp + std::time::Duration::from_secs(1)).unwrap();
        let offered: Input = serde_json::from_value(offered).unwrap();
        let lease = fixture.service.acquire_live_session_actor_turn_boundary_lease(&receiver).await.unwrap();
        let controller = fixture.service.pin_controller_client_for_actor(&lease).await.unwrap();
        drop(lease);
        let runtime = fixture.native.identity_runtime().unwrap();
        let spec = runtime.registered_spec(&AgentIdentity::parse(ADDED).unwrap()).await.unwrap().spec;
        let context = GovernedBindContext { session_id: receiver.clone(), controller, spec };
        let route = fixture.source.route(&after.event.route).unwrap();
        let auth = crate::private_originals::governed_submit::evidence(&route.authentication_domain,
            &after.event.original_authentication).unwrap();
        let peer = after.peer_ingress.as_ref().unwrap();
        let binder = crate::private_originals::governed_submit::OriginalBinder {
            review_custody: crate::review_custody::ReviewCustody::default(),
            source: fixture.source.clone(), grant: None, batch: None, original: reference,
            held: OnceLock::new(), pending: OnceLock::new(), authentication: auth,
            event_key: peer.event_key(), console: None, approved_login: None,
        };
        let bound = binder.bind(&context, offered.clone()).await.unwrap();
        assert!(bound.entry_fence.is_none());
        assert_eq!(serde_json::to_value(&bound.input).unwrap(), serde_json::to_value(&exact).unwrap());
        let current = peer.for_current_offer(bound.input, offered.clone()).unwrap();
        meerkat_runtime::input_authority::check_bound_original(&offered, &current,
            &meerkat_runtime::LogicalRuntimeId::for_session(&receiver),
            meerkat_runtime::input_authority::BoundOriginalPin::DeliveryWork(&after.event.original)).unwrap();
        meerkat_runtime::input_state::verify_exact_replay(&admitted.state, &current).unwrap();
        let calls = fixture.provider.0.load(Ordering::SeqCst);
        let (outcome, completion) = fixture.machine.accept_input_with_completion(&receiver, current).await.unwrap();
        assert!(matches!(outcome, meerkat_runtime::accept::AcceptOutcome::Deduplicated { existing_id, .. }
            if existing_id == admitted.state.input_id));
        assert!(completion.is_none());
        binder.retain_unresolved(bound.settlement, "actual native replay observed");
        assert_eq!(fixture.provider.0.load(Ordering::SeqCst), calls);
        assert_eq!(fixture.source.store.get(&after_row.key).await.unwrap(), Some(after_row));
        assert_eq!(native_input_states(&fixture, &receiver).await.len(), 1);
    }))).catch_unwind().await;
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("queued peer control did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_peer_run_refuses_its_prepared_model_check_without_revoking_grants() {
    let fixture = Box::pin(start()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 8, Box::pin(async {
        let receiver = add(&fixture, ADDED).await;
        let sender = add(&fixture, SENDER).await;
        ready(&fixture, &receiver).await;
        ready(&fixture, &sender).await;
        wire_pair(&fixture).await;
        let recipient = peer(&fixture, ADDED).await;
        *fixture.factory.next_call.lock().unwrap() = None;
        let hold = fixture.provider.hold_next_response();
        fixture.factory.capture_model_check.store(true, Ordering::SeqCst);
        assert!(matches!(send(&fixture, &sender, &recipient).await.unwrap(),
            meerkat_core::SendReceipt::PeerMessageSent { delivery:
                meerkat_core::comms::PeerDeliveryOutcome::DurablyResolved { outcome:
                    meerkat_core::interaction::PeerIngressTerminalOutcomeKind::Accepted }, .. }));
        hold.entered().await;
        let prepared = fixture.factory.captured_model_check.lock().unwrap().take().unwrap();
        prepared.current().expect("the captured decision is current while the actual run is held");
        let rows = native_input_states(&fixture, &receiver).await;
        assert_eq!(rows.len(), 1);
        let input = rows.into_iter().next().unwrap();
        let facts = prepared.binding().facts();
        assert!(matches!(&facts.operation, AuthorizationOperation::Model(_)));
        assert!(facts.run_id.is_some());
        assert_eq!(facts.run_id, input.seed.last_run_id);
        assert!(matches!(&facts.execution_scope,
            meerkat_core::exact_operation::OperationExecutionScope::RuntimeInput {
                owner_session_id, submitted_input_id, canonical_input_id, ..
            } if owner_session_id == &receiver && submitted_input_id == &input.state.input_id
                && canonical_input_id == &input.state.input_id));
        // Promotion may need the live turn boundary, which this response
        // intentionally holds. Read the already-bound canonical association
        // now; await the actual promotion only after releasing the response.
        let own = input.state.authority_contributors.iter()
            .filter(|own| own.input_id() == &input.state.input_id).collect::<Vec<_>>();
        assert_eq!(own.len(), 1);
        let rows = fixture.source.store.list_records(&fixture.source.scope, None, 64).await.unwrap();
        let record = rows.iter().filter(|row| row.data.schema == SOURCE_SCHEMA)
            .map(|row| fixture.source.decode_stored(row).unwrap())
            .find(|record| record.final_association.as_ref() == Some(own[0].association())).unwrap();
        assert!(record.peer_ingress.is_some() && !record.operation_cancelled);
        let admission = fixture.policy.member_class_admission(&instance(ADDED).member.member).unwrap().unwrap();
        let context = fixture.source.callback_grant_context().unwrap();
        let basis = &record.final_association.as_ref().unwrap().candidate().authority_basis;
        let WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } = basis else { panic!("exact delivery targets"); };
        for target in targets {
            context.grants.resolve_lineage(&target.lineage, &admission.route.as_ref().unwrap().logical_executor,
                admission.route.as_ref().unwrap().represented_subject.as_ref()).unwrap().check_current().unwrap();
        }
        hold.release();
        let terminal = completed(&fixture, &receiver, 1).await;
        assert_eq!(terminal.state.input_id, input.state.input_id);
        assert_eq!(terminal.seed.last_run_id, facts.run_id);
        assert_eq!(terminal.seed.terminal_outcome, Some(meerkat_runtime::input_state::InputTerminalOutcome::Consumed));
        let (settled_row, settled_source, _) = source_row(&fixture, &terminal).await;
        assert!(settled_source.event == record.event);
        assert!(settled_source.peer_ingress == record.peer_ingress);
        assert_eq!(settled_source.final_association, record.final_association);
        // Currentness must fail because the actual owning run completed, while
        // the runtime, source and durable grants remain present and current.
        assert!(matches!(prepared.current(), Err(meerkat_core::OperationAuthorizationError::Refused(error))
            if error.kind() == OperationRefusalKind::MalformedFacts));
        assert_eq!(fixture.source.store.get(&settled_row.key).await.unwrap(), Some(settled_row));
        assert_eq!(fixture.policy.member_class_admission(&instance(ADDED).member.member).unwrap(), Some(admission.clone()));
        for target in targets {
            context.grants.resolve_lineage(&target.lineage, &admission.route.as_ref().unwrap().logical_executor,
                admission.route.as_ref().unwrap().represented_subject.as_ref()).unwrap().check_current().unwrap();
            for reference in &target.lineage {
                assert_eq!(context.grants.issued_reference(&reference.grant_id).unwrap().as_ref(), Some(reference));
            }
        }
        assert_eq!(native_input_states(&fixture, &receiver).await.len(), 1);
        assert!(native_input_states(&fixture, &sender).await.is_empty());
        assert!(fixture.factory.calls.lock().unwrap().is_empty());
    }))).catch_unwind().await;
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("completed-run model control did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_host_restores_exact_peer_delivery_and_admits_new_receiver_work() {
    let fixture = Box::pin(Fixture::start_durable_with_recorder(receiver_class())).await;
    fixture
        .source
        .start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer())
        .unwrap();
    let prepared = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 8,
        Box::pin(async {
            let receiver = add(&fixture, ADDED).await;
            let sender = add(&fixture, SENDER).await;
            ready(&fixture, &receiver).await;
            ready(&fixture, &sender).await;
            wire_pair(&fixture).await;
            let recipient = peer(&fixture, ADDED).await;
            *fixture.factory.next_call.lock().unwrap() = Some("memory");
            *fixture.factory.next_args.lock().unwrap() = serde_json::json!({"action":"remember",
            "title":"Before close", "body":"Actual first host Peer delivery"});
            assert!(matches!(
                send(&fixture, &sender, &recipient).await.unwrap(),
                meerkat_core::SendReceipt::PeerMessageSent {
                    delivery: meerkat_core::comms::PeerDeliveryOutcome::DurablyResolved {
                        outcome:
                            meerkat_core::interaction::PeerIngressTerminalOutcomeKind::Accepted
                    },
                    ..
                }
            ));
            let input = completed(&fixture, &receiver, 1).await;
            tool_succeeded(&input, "memory");
            let (row, record, reference) = source_row(&fixture, &input).await;
            let admission = fixture
                .policy
                .member_class_admission(&instance(ADDED).member.member)
                .unwrap()
                .unwrap();
            assert_eq!(fixture.roster.snapshot().len(), 3);
            (
                receiver, sender, recipient, input, row, record, reference, admission,
            )
        }),
    ))
    .catch_unwind()
    .await;
    let (receiver, sender, recipient, input, row, record, reference, admission) = match prepared {
        Ok(Ok(value)) => value,
        failed => {
            fixture.close().await;
            match failed {
                Err(panic) => std::panic::resume_unwind(panic),
                _ => panic!("cold Peer preparation did not settle"),
            }
        }
    };
    let old_source = Arc::downgrade(&fixture.source);
    let old_lifetime = Arc::downgrade(&fixture.lifetime);
    let old_factory = Arc::downgrade(&fixture.factory);
    let old_machine = Arc::downgrade(&fixture.machine);
    // Existing fixture close joins native and installed owners; construction
    // after their drop reopens the actual SQLite stores and retained roster.
    let fixture = fixture.reopen_with_recorder().await;
    // Use the existing recovery fixture's first-round hold. The real worker
    // is selected, but cannot race manual native replay for the source writer.
    // It resumes before the new delivery and on every cleanup path.
    let mut first_cold_round = Some(fixture.source.hold_first_custody_round_for_test());
    fixture
        .source
        .start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer())
        .unwrap();
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 8, Box::pin(async {
        assert!(old_source.upgrade().is_none() && old_lifetime.upgrade().is_none()
            && old_factory.upgrade().is_none() && old_machine.upgrade().is_none());
        ready(&fixture, &receiver).await;
        ready(&fixture, &sender).await;
        // Read the durable native edge under fresh owners; do not wire again.
        assert_pair_wired(&fixture).await;
        assert_eq!(peer(&fixture, ADDED).await, recipient);
        assert_eq!(fixture.policy.member_class_admission(&instance(ADDED).member.member).unwrap(), Some(admission));
        let restored_rows = native_input_states(&fixture, &receiver).await;
        assert_eq!(restored_rows.len(), 1, "Restore cannot create a replacement input");
        let restored = &restored_rows[0];
        assert_eq!(restored.state.input_id, input.state.input_id);
        let exact = cold_service_admission::decode_source_input(&record, record.bound_service_input.as_ref().unwrap()).unwrap();
        assert!(matches!(&exact, Input::Peer(_)));
        meerkat_runtime::input_state::verify_exact_replay(&restored.state, &exact).unwrap();
        let (restored_row, restored_source, _) = source_row(&fixture, restored).await;
        assert_eq!(restored_row, row);
        assert_eq!(restored_source.final_association, record.final_association);
        let context = fixture.source.callback_grant_context().unwrap();
        let restored_admission = fixture.policy.member_class_admission(&instance(ADDED).member.member).unwrap().unwrap();
        let WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } =
            &record.final_association.as_ref().unwrap().candidate().authority_basis else { panic!("exact delivery targets"); };
        for target in targets {
            context.grants.resolve_lineage(&target.lineage,
                &restored_admission.route.as_ref().unwrap().logical_executor,
                restored_admission.route.as_ref().unwrap().represented_subject.as_ref()).unwrap().check_current().unwrap();
            for reference in &target.lineage {
                assert_eq!(context.grants.issued_reference(&reference.grant_id).unwrap().as_ref(), Some(reference));
            }
        }
        // This is actual admitted-original recovery and native replay under
        // fresh owners, not a second physical delivery or a synthesized row.
        use meerkat_mobkit::identity_first::{GovernedBindContext, GovernedInputBinder};
        let lease = fixture.service.acquire_live_session_actor_turn_boundary_lease(&receiver).await.unwrap();
        let controller = fixture.service.pin_controller_client_for_actor(&lease).await.unwrap();
        drop(lease);
        let runtime = fixture.native.identity_runtime().unwrap();
        let spec = runtime.registered_spec(&AgentIdentity::parse(ADDED).unwrap()).await.unwrap().spec;
        let bind_context = GovernedBindContext { session_id: receiver.clone(), controller, spec };
        let route = fixture.source.route(&record.event.route).unwrap();
        let auth = crate::private_originals::governed_submit::evidence(&route.authentication_domain,
            &record.event.original_authentication).unwrap();
        let ingress = record.peer_ingress.as_ref().unwrap();
        let binder = crate::private_originals::governed_submit::OriginalBinder {
            review_custody: crate::review_custody::ReviewCustody::default(),
            source: fixture.source.clone(), grant: None, batch: None, original: reference,
            held: OnceLock::new(), pending: OnceLock::new(), authentication: auth,
            event_key: ingress.event_key(), console: None, approved_login: None,
        };
        let mut offered = serde_json::to_value(&exact).unwrap();
        offered["header"].as_object_mut().unwrap().remove("authority_association");
        offered["header"]["timestamp"] = serde_json::to_value(
            exact.header().timestamp + std::time::Duration::from_secs(1)).unwrap();
        let offered: Input = serde_json::from_value(offered).unwrap();
        let bound = binder.bind(&bind_context, offered.clone()).await.unwrap();
        assert!(bound.entry_fence.is_none());
        assert_eq!(serde_json::to_value(&bound.input).unwrap(), serde_json::to_value(&exact).unwrap());
        let current = ingress.for_current_offer(bound.input, offered.clone()).unwrap();
        meerkat_runtime::input_authority::check_bound_original(&offered, &current,
            &meerkat_runtime::LogicalRuntimeId::for_session(&receiver),
            meerkat_runtime::input_authority::BoundOriginalPin::DeliveryWork(&record.event.original)).unwrap();
        let calls = fixture.provider.0.load(Ordering::SeqCst);
        let (outcome, completion) = fixture.machine.accept_input_with_completion(&receiver, current).await.unwrap();
        assert!(matches!(outcome, meerkat_runtime::accept::AcceptOutcome::Deduplicated { existing_id, .. }
            if existing_id == input.state.input_id));
        assert!(completion.is_none());
        binder.retain_unresolved(bound.settlement, "actual cold native replay observed");
        assert_eq!(fixture.provider.0.load(Ordering::SeqCst), calls);
        assert_eq!(fixture.source.store.get(&row.key).await.unwrap(), Some(row));
        assert_eq!(native_input_states(&fixture, &receiver).await.len(), 1);
        drop(binder);
        first_cold_round.take().unwrap().send(()).unwrap();
        *fixture.factory.next_call.lock().unwrap() = Some("memory");
        *fixture.factory.next_args.lock().unwrap() = serde_json::json!({"action":"remember",
            "title":"After reopen", "body":"Actual fresh host Peer delivery"});
        let observed_before = fixture.peer_observer.offers.lock().unwrap().len();
        assert!(matches!(send(&fixture, &sender, &recipient).await.unwrap(),
            meerkat_core::SendReceipt::PeerMessageSent { delivery:
                meerkat_core::comms::PeerDeliveryOutcome::DurablyResolved { outcome:
                    meerkat_core::interaction::PeerIngressTerminalOutcomeKind::Accepted }, .. }));
        // Admission sequences order a live driver, not historical rows across
        // reopen. Select this delivery's actual Core offer before its receipt.
        let next_offer = {
            let offers = fixture.peer_observer.offers.lock().unwrap();
            let fresh = &offers[observed_before..];
            let offered = fresh.iter().find(|(session, _, not_ready)|
                session == &receiver && !not_ready).expect("accepted fresh Core offer").1.clone();
            assert!(fresh.iter().all(|(session, input, _)|
                session == &receiver && input.id() == offered.id()), "one fresh delivery identity");
            offered
        };
        let receipt = tokio::time::timeout(WAIT,
            fixture.machine.wait_input_terminal_receipt(&receiver, next_offer.id())).await.unwrap().unwrap();
        assert!(matches!(receipt, Some(meerkat_runtime::terminal_status::InputTerminalReceiptWait::Resolved(_))));
        let rows = native_input_states(&fixture, &receiver).await;
        assert_eq!(rows.len(), 2, "the retained and fresh native inputs must both remain");
        assert!(rows.iter().any(|row| row.state.input_id == input.state.input_id));
        let mut matching = rows.into_iter().filter(|row| &row.state.input_id == next_offer.id());
        let next = matching.next().expect("the actual fresh offer must have a native row");
        assert!(matching.next().is_none(), "one row for the fresh native input");
        tool_succeeded(&next, "memory");
        let (_, next_source, _) = source_row(&fixture, &next).await;
        next_source.peer_ingress.as_ref().unwrap().check_input(&next_offer, false).unwrap();
        assert_ne!(next.state.input_id, input.state.input_id);
        assert_ne!(next_source.event.original, record.event.original);
        assert_ne!(next_source.final_association, record.final_association);
        use meerkat_mobkit::identity_first::agent_memory::{AgentMemoryProvider, AgentMemorySelection, AgentMemoryRecallRequest};
        let memory = meerkat_mobkit::memory::SqliteAgentMemoryStore::open(fixture._directory.path().join("selected-recorder")).unwrap();
        let records = memory.recall(AgentMemoryRecallRequest {
            realm: fixture.lifetime.selected_policy().native_host.as_ref().unwrap().realm.to_string(),
            identity: AgentIdentity::parse(ADDED).unwrap(), query_text: None, query_terms: vec![],
            selection: AgentMemorySelection::Always, max_entries: 16,
        }).await.unwrap();
        assert_eq!(records.len(), 2);
        for body in ["Actual first host Peer delivery", "Actual fresh host Peer delivery"] {
            assert!(records.iter().any(|record| record.body == body));
        }
        assert!(native_input_states(&fixture, &sender).await.is_empty());
        assert!(fixture.factory.calls.lock().unwrap().is_empty());
    }))).catch_unwind().await;
    drop(first_cold_round.take());
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("cold Peer control did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

// Capture only the native owned ticket. The call and plan stay inside Core;
// reconstructing their bytes would test pointer mismatch instead of lifetime.
pub(super) struct RecorderTicketCapture {
    enabled: AtomicBool,
    ticket: Mutex<Option<meerkat_core::ReviewedEntryTicket>>,
    run: Mutex<Option<meerkat_core::RunId>>,
    after_effect: tokio::sync::Notify,
    release: tokio::sync::watch::Sender<bool>,
    outcomes: AtomicUsize,
}
impl RecorderTicketCapture {
    pub(super) fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            ticket: Mutex::new(None),
            run: Mutex::new(None),
            after_effect: tokio::sync::Notify::new(),
            release: tokio::sync::watch::channel(false).0,
            outcomes: AtomicUsize::new(0),
        }
    }
}
#[async_trait::async_trait]
impl meerkat_core::ToolDispatchAdmission for RecorderTicketCapture {
    async fn await_dispatch_admission(
        &self,
        call: meerkat_core::ToolCallView<'_>,
        context: Option<&meerkat_core::ToolDispatchContext>,
        _kind: meerkat_core::LiveBridgeEffectKind,
    ) -> Result<(), meerkat_core::ToolError> {
        assert_eq!(call.name, "memory");
        let context = context.expect("actual governed Recorder dispatch context");
        assert!(context.work_authorization().is_some());
        context.check_tool_authorization(call, None)?;
        let ticket = context.reviewed_entry_ticket(call, None)?;
        assert!(self.ticket.lock().unwrap().replace(ticket).is_none());
        *self.run.lock().unwrap() = Some(context.run_id().expect("actual native run").clone());
        Ok(())
    }
    async fn record_dispatch_outcome(
        &self,
        call: meerkat_core::ToolCallView<'_>,
        context: Option<&meerkat_core::ToolDispatchContext>,
        _kind: meerkat_core::LiveBridgeEffectKind,
        outcome: meerkat_core::LiveBridgeEffectOutcome,
    ) -> Result<(), meerkat_core::ToolError> {
        assert_eq!(call.name, "memory");
        assert_eq!(outcome, meerkat_core::LiveBridgeEffectOutcome::Committed);
        // The real Recorder body has returned. Its exact prepared check is
        // still current; the stored single-use ticket has never been entered.
        context.unwrap().check_tool_authorization(call, None)?;
        assert_eq!(self.outcomes.fetch_add(1, Ordering::SeqCst), 0);
        self.after_effect.notify_one();
        let mut release = self.release.subscribe();
        while !*release.borrow_and_update() {
            release.changed().await.unwrap();
        }
        Ok(())
    }
}

pub(super) fn observe_recorder_ticket(
    inner: Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>,
    capture: Arc<RecorderTicketCapture>,
) -> Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer> {
    Arc::new(RecorderTicketCustomizer { inner, capture })
}
struct RecorderTicketCustomizer {
    inner: Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>,
    capture: Arc<RecorderTicketCapture>,
}
#[async_trait::async_trait]
impl meerkat_mobkit::identity_first::AgentCustomizer for RecorderTicketCustomizer {
    async fn customize_build(
        &self,
        context: &meerkat_mobkit::identity_first::AgentBuildContext,
        spec: &DurableAgentSpec,
        draft: &mut meerkat_mobkit::identity_first::AgentBuildDraft,
    ) -> Result<(), meerkat_mobkit::identity_first::CustomizerError> {
        self.inner.customize_build(context, spec, draft).await?;
        if self.capture.enabled.load(Ordering::SeqCst)
            && context.identity == AgentIdentity::parse(ADDED).unwrap()
        {
            let recorder = draft
                .local_external_tools
                .dispatcher()
                .expect("actual Meta Recorder overlay");
            // Core's existing gate forwards the complete tool/target surface.
            // This additional neutral gate does not replace the installed
            // class/access/consequence gates wrapped around the overlay later.
            draft.local_external_tools =
                meerkat_mobkit::identity_first::LocalExternalToolOverlay::new(Arc::new(
                    meerkat_core::ExecutionPolicyGatedDispatcher::new(
                        recorder,
                        meerkat_core::ToolExecutionPolicy::unrestricted(),
                    )
                    .with_dispatch_admission(self.capture.clone()),
                ));
        }
        Ok(())
    }
    async fn after_create(
        &self,
        identity: &AgentIdentity,
        session: &meerkat_core::SessionId,
        context: &meerkat_mobkit::SessionCreatedContext,
    ) -> Result<(), meerkat_mobkit::identity_first::CustomizerError> {
        self.inner.after_create(identity, session, context).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_peer_run_refuses_unused_recorder_effect_ticket_without_revoking_grants() {
    let fixture = Box::pin(start()).await;
    let capture = fixture.factory.recorder_ticket.clone();
    capture.enabled.store(true, Ordering::SeqCst);
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 8, Box::pin(async {
        let receiver = add(&fixture, ADDED).await;
        let sender = add(&fixture, SENDER).await;
        ready(&fixture, &receiver).await;
        ready(&fixture, &sender).await;
        wire_pair(&fixture).await;
        let recipient = peer(&fixture, ADDED).await;
        *fixture.factory.next_call.lock().unwrap() = Some("memory");
        *fixture.factory.next_args.lock().unwrap() = serde_json::json!({"action":"remember",
            "title":"Retained effect ticket", "body":"Exactly one actual Recorder write"});
        assert!(matches!(send(&fixture, &sender, &recipient).await.unwrap(),
            meerkat_core::SendReceipt::PeerMessageSent { delivery:
                meerkat_core::comms::PeerDeliveryOutcome::DurablyResolved { outcome:
                    meerkat_core::interaction::PeerIngressTerminalOutcomeKind::Accepted }, .. }));
        capture.after_effect.notified().await;
        let run = capture.run.lock().unwrap().clone().unwrap();
        let ticket = capture.ticket.lock().unwrap().take().unwrap();
        let rows = native_input_states(&fixture, &receiver).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].seed.last_run_id.as_ref(), Some(&run));
        assert!(rows[0].seed.terminal_outcome.is_none(), "the actual post-effect dispatch is still held");
        use meerkat_mobkit::identity_first::agent_memory::{AgentMemoryProvider, AgentMemorySelection, AgentMemoryRecallRequest};
        let memory = meerkat_mobkit::memory::SqliteAgentMemoryStore::open(fixture._directory.path().join("selected-recorder")).unwrap();
        let query = AgentMemoryRecallRequest {
            realm: fixture.lifetime.selected_policy().native_host.as_ref().unwrap().realm.to_string(),
            identity: AgentIdentity::parse(ADDED).unwrap(), query_text: None, query_terms: vec![],
            selection: AgentMemorySelection::Always, max_entries: 16,
        };
        let before = memory.recall(query.clone()).await.unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].body, "Exactly one actual Recorder write");
        let admission = fixture.policy.member_class_admission(&instance(ADDED).member.member).unwrap().unwrap();
        let grants = fixture.source.callback_grant_context().unwrap();
        let own = rows[0].state.authority_contributors.iter()
            .filter(|own| own.input_id() == &rows[0].state.input_id).collect::<Vec<_>>();
        assert_eq!(own.len(), 1);
        let association = own[0].association().clone();
        let WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } = &association.candidate().authority_basis
            else { panic!("actual delivery target lineages"); };
        for target in targets {
            grants.grants.resolve_lineage(&target.lineage, &admission.route.as_ref().unwrap().logical_executor,
                admission.route.as_ref().unwrap().represented_subject.as_ref()).unwrap().check_current().unwrap();
        }
        capture.release.send_replace(true);
        let terminal = completed(&fixture, &receiver, 1).await;
        assert_eq!(terminal.seed.last_run_id.as_ref(), Some(&run));
        assert_eq!(terminal.seed.terminal_outcome, Some(meerkat_runtime::input_state::InputTerminalOutcome::Consumed));
        tool_succeeded(&terminal, "memory");
        let (settled_row, settled, _) = source_row(&fixture, &terminal).await;
        assert_eq!(settled.final_association.as_ref(), Some(&association));
        // Its first and only enter is now. This checks the composed native
        // path refuses late Recorder entry, not one specific run fence.
        // After a publication change, re-preparation can also reject archived
        // input bindings as MalformedFacts. Isolated run-fence coverage belongs
        // to Core; keep the effect and unchanged-grant assertions here.
        assert!(matches!(ticket.enter(), Err(meerkat_core::ToolError::AuthorizationRefused { refusal })
            if refusal.kind() == OperationRefusalKind::MalformedFacts));
        assert_eq!(memory.recall(query).await.unwrap(), before);
        assert_eq!(capture.outcomes.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.source.store.get(&settled_row.key).await.unwrap(), Some(settled_row));
        assert_eq!(fixture.policy.member_class_admission(&instance(ADDED).member.member).unwrap(), Some(admission.clone()));
        for target in targets {
            grants.grants.resolve_lineage(&target.lineage, &admission.route.as_ref().unwrap().logical_executor,
                admission.route.as_ref().unwrap().represented_subject.as_ref()).unwrap().check_current().unwrap();
            for reference in &target.lineage {
                assert_eq!(grants.grants.issued_reference(&reference.grant_id).unwrap().as_ref(), Some(reference));
            }
        }
        assert_eq!(native_input_states(&fixture, &receiver).await.len(), 1);
        assert!(native_input_states(&fixture, &sender).await.is_empty());
        assert!(fixture.factory.calls.lock().unwrap().is_empty());
    }))).catch_unwind().await;
    // Release an outcome callback even if a preceding assertion failed; native
    // shutdown then joins the real dispatch and the existing fixture owners.
    capture.release.send_replace(true);
    drop(capture.ticket.lock().unwrap().take());
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("Recorder ticket control did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

//! Real Core comms ingress into actual Added receiver sessions. No constructed
//! binder request, imported sender permission, or synthetic admission proof.
use super::native_operations_tests::{native_input_states, ready};
use super::*;
use meerkat_authorization_contracts::audit::{
    AuditObservation, AuditTarget, StoredAuthorizationAuditObservation,
};
use meerkat_authorization_contracts::work_association::{
    GrantTarget, InputAuthorityAssociation, InputAuthorityAssociationCandidate, OriginalWorkRef,
};
use meerkat_core::authorization::OperationObservedOutcome;
use meerkat_runtime::input_authority::{
    BoundOriginalPin, GovernedPeerOrigin, PeerIngressBindDenial, PeerIngressBindRefusal,
    PeerIngressOrigin,
};
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
/// fixture never constructs a request or overrides a binding/refusal. It
/// records the governed origin of every offer; a row may substitute a real
/// origin it recorded earlier into later governed offers to one receiving
/// session, before the production binder reads them. Core still pins its own
/// copy of each delivery's true origin.
pub(super) struct ObservedReceiver {
    source: Weak<PrivateOriginals>,
    offers: Mutex<Vec<(meerkat_core::SessionId, Input, bool)>>,
    declined: tokio::sync::Notify,
    origins: Mutex<Vec<(meerkat_core::SessionId, Option<GovernedPeerOrigin>)>>,
    substitute: Mutex<Option<(meerkat_core::SessionId, GovernedPeerOrigin)>>,
    substituted: Mutex<Vec<Result<(), PeerIngressBindRefusal>>>,
}
impl ObservedReceiver {
    pub(super) fn new(source: Arc<PrivateOriginals>) -> Self {
        Self {
            source: Arc::downgrade(&source),
            offers: Mutex::new(vec![]),
            declined: tokio::sync::Notify::new(),
            origins: Mutex::new(vec![]),
            substitute: Mutex::new(None),
            substituted: Mutex::new(vec![]),
        }
    }

    /// The governed origins of the actual offers to `session`, in order.
    fn governed_origins(&self, session: &meerkat_core::SessionId) -> Vec<GovernedPeerOrigin> {
        self.origins
            .lock()
            .unwrap()
            .iter()
            .filter(|(offered, _)| offered == session)
            .filter_map(|(_, origin)| origin.clone())
            .collect()
    }

    /// Record the offer's governed origin, then put the armed substitute in
    /// its place when the offer is governed and addressed to its session.
    fn observe_origin(
        &self,
        session: &meerkat_core::SessionId,
        request: &mut meerkat_runtime::input_authority::PeerIngressBindRequest,
    ) -> bool {
        let PeerIngressOrigin::Comms(comms) = &mut request.origin else {
            return false;
        };
        let mut origins = self.origins.lock().unwrap();
        assert!(origins.len() < 128, "bounded actual peer origins");
        origins.push((session.clone(), comms.governed_origin.clone()));
        drop(origins);
        if comms.governed_origin.is_none() {
            return false;
        }
        match &*self.substitute.lock().unwrap() {
            Some((target, origin)) if target == session => {
                comms.governed_origin = Some(origin.clone());
                true
            }
            _ => false,
        }
    }
}
impl meerkat_runtime::input_authority::PeerIngressBinder for ObservedReceiver {
    fn bind_peer_ingress(
        &self,
        mut request: meerkat_runtime::input_authority::PeerIngressBindRequest,
    ) -> meerkat_runtime::input_authority::PeerIngressBindFuture<'_> {
        Box::pin(async move {
            let session = request.session_id.clone();
            let input = request.input.clone();
            let substituted = self.observe_origin(&session, &mut request);
            let source = self.source.upgrade().ok_or_else(|| {
                meerkat_runtime::input_authority::PeerIngressBindRefusal::Unavailable(
                    "fixture source closed".into(),
                )
            })?;
            let result = source.bind_peer_delivery(request).await;
            if substituted {
                self.substituted
                    .lock()
                    .unwrap()
                    .push(result.as_ref().map(|_| ()).map_err(Clone::clone));
            }
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
    if let Err(failure) = tool_succeeded_in(&audit, name) {
        panic!(
            "{failure}: {}",
            serde_json::to_string(
                &audit
                    .iter()
                    .map(|row| &row.observation.observation)
                    .collect::<Vec<_>>()
            )
            .unwrap()
        );
    }
}

/// Whether the native tool `name` prepared once in `audit` and returned a
/// success, with no refused or unavailable answer for its operation but
/// Core's own refusal to rebuild a preparation that met an owner change
/// (`ReprepareRequired`, read while the publication changed), recorded
/// before the operation's preparation: Core waits that change out and
/// prepares the operation again under the same id.
fn tool_succeeded_in(
    audit: &[StoredAuthorizationAuditObservation],
    name: &str,
) -> Result<(), String> {
    let operations = audit.iter().filter_map(|row| match &row.observation.observation {
        AuditObservation::Prepared { target, .. }
            if matches!(target.as_ref(), AuditTarget::Tool { tool_name, .. } if tool_name == name)
            => Some(row.observation.operation_id.clone()),
        _ => None,
    }).collect::<std::collections::HashSet<_>>();
    if operations.len() != 1 {
        return Err(format!(
            "the actual selected native tool must prepare once: {} operations",
            operations.len()
        ));
    }
    if !audit.iter().any(|row| {
        operations.contains(&row.observation.operation_id)
            && matches!(
                &row.observation.observation,
                AuditObservation::Outcome {
                    outcome: OperationObservedOutcome::ToolDispatchReturned {
                        result_is_error: false,
                        terminal_error: None,
                        ..
                    }
                }
            )
    }) {
        return Err("the tool's operation returned no success".to_string());
    }
    let prepared_at = audit.iter().position(|row| {
        operations.contains(&row.observation.operation_id)
            && matches!(
                row.observation.observation,
                AuditObservation::Prepared { .. }
            )
    });
    let refused = audit
        .iter()
        .enumerate()
        .filter(|(at, row)| {
            operations.contains(&row.observation.operation_id)
                && match &row.observation.observation {
                    AuditObservation::Refused { reason, read, .. } => {
                        // Core's waited re-preparation: refused to rebuild at a
                        // read the publication changed under, before the
                        // operation's preparation.
                        let reprepared = *reason
                            == meerkat_core::authorization::OperationRefusalKind::ReprepareRequired
                            && read.as_ref().is_some_and(|read| {
                                matches!(
                                    read.basis,
                                    meerkat_authorization_contracts::audit::AuditRefusalBasis::ChangeInProgress
                                        | meerkat_authorization_contracts::audit::AuditRefusalBasis::PublicationChanged
                                )
                            })
                            && prepared_at.is_some_and(|prepared| *at < prepared);
                        !reprepared
                    }
                    AuditObservation::AuthorizationUnavailable { .. } => true,
                    _ => false,
                }
        })
        .map(|(_, row)| row)
        .collect::<Vec<_>>();
    if !refused.is_empty() {
        return Err(format!(
            "the tool's operation was refused or unavailable before it succeeded: {refused:?}"
        ));
    }
    Ok(())
}

/// One stored audit row of the checked input, for operation `operation`, as
/// Core records it for a peer delivery's memory tool (the shape of a run's
/// recorded rows).
fn audit_row(operation: &str, observation: serde_json::Value) -> serde_json::Value {
    const INPUT: &str = "eaed4620-bd71-84db-b5a9-a4d2f8af4e8b";
    let principal = |id: &str, kind: &str, domain: &str| {
        serde_json::json!({
            "id": id,
            "kind": kind,
            "qualification": {"kind": "qualified", "trust_domain_id": domain},
        })
    };
    let human = principal("uid-1001", "human", "rkat-realm-mob.factory");
    serde_json::json!({
        "contributors": [{
            "input_id": INPUT,
            "logical_executor": principal("project-one", "shared_agent", "factory-test"),
            "represented_subject": human.clone(),
            "requester": human,
        }],
        "observation": {
            "context_revision": null,
            "execution_scope": {
                "canonical_input_id": INPUT,
                "kind": "runtime_input",
                "owner_session_id": "01a1b7f5-bc68-70ac-9aa8-09068e6e83ff",
                "runtime_epoch_id": "01a1b7f5-bc6a-77c4-b5e5-5984a414cb05",
                "submitted_input_id": INPUT,
            },
            "observation": observation,
            "operation_id": operation,
            "run_id": "01a1b7f5-c1e7-772c-9d18-6c987dd9dc97",
        },
    })
}

/// The memory tool call an audit row of [`audit_row`] names.
fn memory_tool() -> serde_json::Value {
    let arguments_digest = vec![0_u8; 32];
    serde_json::json!({
        "arguments_digest": arguments_digest,
        "call_id": "project-read-call",
        "kind": "tool",
        "owners": [{"authority_key": "root-dispatcher", "owner_key": "memory"}],
        "tool_name": "memory",
    })
}

const PUBLICATION: &str = "bfd436ef-da2f-496a-8166-b11e090236d4";

/// A refusal of the memory tool's operation, of `reason`, decided while an
/// owner change was in progress on the publication.
fn refused_in_a_change(reason: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "refused",
        "read": {
            "basis": "change_in_progress",
            "live_sequence": 65,
            "observed_at_ms": 1_794_087_699_016_u64,
            "publication_instance": PUBLICATION,
            "publication_sequence": 65,
        },
        "reason": reason,
        "target": memory_tool(),
    })
}

fn prepared_memory_tool() -> serde_json::Value {
    serde_json::json!({
        "kind": "prepared",
        "policy": {
            "controller_lineages": [],
            "observed_at_ms": 1_794_087_699_018_u64,
            "operation_authorities": [],
            "publication_instance": PUBLICATION,
            "publication_sequence": 66,
        },
        "target": memory_tool(),
    })
}

fn entered(boundary: &str) -> serde_json::Value {
    serde_json::json!({"boundary": boundary, "kind": "entry"})
}

fn returned_success() -> serde_json::Value {
    serde_json::json!({
        "kind": "outcome",
        "outcome": {
            "asynchronous_operations": [],
            "kind": "tool_dispatch_returned",
            "result_is_error": false,
            "terminal_error": null,
        },
    })
}

fn decoded(rows: Vec<serde_json::Value>) -> Vec<StoredAuthorizationAuditObservation> {
    serde_json::from_value(serde_json::Value::Array(rows)).unwrap()
}

/// Core's waited re-preparation, as a peer delivery's memory tool met it:
/// the tool's first preparation met an owner change in progress on the
/// authorization publication and was refused `ReprepareRequired` before any
/// preparation was recorded; Core waited the change out and prepared it
/// again under the same operation id, and the tool ran once and succeeded.
/// The success check accepts that refusal, and still refuses the same
/// refusal after the operation's preparation, a refusal of another kind and
/// an unavailable answer.
///
/// RED before the fix: the check refuses Core's own refusal to rebuild.
#[test]
fn a_tool_prepared_again_after_an_owner_change_still_succeeded() {
    const TOOL: &str = "01a1b7f5-c23c-77e9-928e-feb1626e6a75";
    let row = |observation: serde_json::Value| audit_row(TOOL, observation);
    let reprepared = vec![
        row(refused_in_a_change("reprepare_required")),
        row(prepared_memory_tool()),
        row(entered("tool_dispatch")),
        row(entered("tool_execution_policy")),
        row(returned_success()),
    ];
    if let Err(failure) = tool_succeeded_in(&decoded(reprepared), "memory") {
        panic!("Core's waited re-preparation is a success: {failure}");
    }
    let unavailable = serde_json::json!({
        "kind": "authorization_unavailable",
        "target": memory_tool(),
    });
    for (what, rows) in [
        (
            "the same refusal after the operation's preparation",
            vec![
                row(prepared_memory_tool()),
                row(refused_in_a_change("reprepare_required")),
                row(returned_success()),
            ],
        ),
        (
            "a refusal of another kind",
            vec![
                row(refused_in_a_change("denied")),
                row(prepared_memory_tool()),
                row(returned_success()),
            ],
        ),
        (
            "an unavailable answer",
            vec![
                row(unavailable),
                row(prepared_memory_tool()),
                row(returned_success()),
            ],
        ),
    ] {
        assert!(
            tool_succeeded_in(&decoded(rows), "memory").is_err(),
            "{what} is never a success"
        );
    }
}

/// The input's own retained association, its canonical source row once the
/// recovery worker promoted it, and its reference. `contributors` is the
/// exact sending work the binding must name: none for a host trigger, the
/// sending work for a delivery that governed work sent.
async fn source_row(
    fixture: &Fixture,
    input: &StoredInputState,
    contributors: &[OriginalWorkRef],
) -> (StoredRecord, SourceRecord, PrivateOriginalRef) {
    let own = input
        .state
        .authority_contributors
        .iter()
        .filter(|own| own.input_id() == &input.state.input_id)
        .collect::<Vec<_>>();
    assert_eq!(own.len(), 1);
    let association = own[0].association();
    assert_eq!(
        association.candidate().contributing_work,
        contributors,
        "the binding names exactly the sending work as its contributors"
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
        let (first_row, first_source, first_ref) = source_row(&fixture, &first, &[]).await;
        let first_binding = first_source.final_association.as_ref().unwrap();
        // A host trigger carries no governed origin: the receiver route's own
        // requester and ingress actor, with no contributor (source_row).
        let receiver_route = fixture.source.route(&first_source.event.route).unwrap();
        assert_eq!(first_binding.candidate().requester, receiver_route.requester);
        assert_eq!(first_binding.candidate().ingress_actor, receiver_route.ingress_actor);
        // The reply was sent from that governed work. Its binding at the sender
        // keeps that work's requester, names the replying agent as its ingress
        // actor and that work as its contributor.
        let (_, reply_source, _) = source_row(&fixture, &reply,
            std::slice::from_ref(&first_binding.candidate().original_work)).await;
        let reply_binding = reply_source.final_association.as_ref().unwrap().candidate();
        assert_eq!(reply_binding.requester, first_binding.candidate().requester);
        assert_eq!(reply_binding.ingress_actor, first_binding.candidate().logical_executor);
        // Every offer of the reply carried its one typed origin. How many offers
        // it took before its binding was admitted (each pending answer is one
        // retry) is measured, not pinned.
        let reply_origins = fixture.peer_observer.governed_origins(&sender);
        assert!(!reply_origins.is_empty() && reply_origins.windows(2).all(|pair| pair[0] == pair[1]));
        eprintln!("governed reply offers at the sender: {}", reply_origins.len());
        // The rest stays the receiving member's own: its executor, the subject it
        // acts for, and its own exact delivery targets.
        let sender_route = fixture.source.route(&reply_source.event.route).unwrap();
        assert_eq!(reply_binding.logical_executor, sender_route.logical_executor);
        assert_eq!(reply_binding.represented_subject, sender_route.represented_subject);
        let WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: reply_targets } =
            &reply_binding.authority_basis else { panic!("the reply must have only exact targets"); };
        assert_eq!(reply_targets.len(), 2);
        assert!(reply_targets.iter().any(|row| row.target == GrantTarget::Peer { peer: receiver_peer }));
        let basis = &first_binding.candidate().authority_basis;
        let WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } = basis else { panic!("delivery must have only exact targets"); };
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|row| row.target == GrantTarget::Peer { peer: sender_peer }));
        assert!(targets.iter().all(|row| !matches!(row.target, GrantTarget::PeerDirectory)));
        assert_ne!(reply_binding.logical_executor, first_binding.candidate().logical_executor);
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
        let (second_row, second_source, second_ref) = source_row(&fixture, &second, &[]).await;
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

fn durably_accepted(receipt: &meerkat_core::SendReceipt) -> bool {
    matches!(
        receipt,
        meerkat_core::SendReceipt::PeerMessageSent {
            delivery: meerkat_core::comms::PeerDeliveryOutcome::DurablyResolved {
                outcome: meerkat_core::interaction::PeerIngressTerminalOutcomeKind::Accepted
            },
            ..
        }
    )
}

/// Wait for the terminal receipt of every native input of `session`; their
/// count.
async fn settled(fixture: &Fixture, session: &meerkat_core::SessionId) -> usize {
    let rows = native_input_states(fixture, session).await;
    for row in &rows {
        let receipt = tokio::time::timeout(
            WAIT,
            fixture
                .machine
                .wait_input_terminal_receipt(session, &row.state.input_id),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(
            receipt,
            Some(meerkat_runtime::terminal_status::InputTerminalReceiptWait::Resolved(_))
        ));
    }
    rows.len()
}

/// The named tool prepared once in `row` and reported no delivered result.
fn tool_not_succeeded(row: &StoredInputState, name: &str) {
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
            .all(|row| !operations.contains(&row.observation.operation_id)
                || !matches!(
                    &row.observation.observation,
                    AuditObservation::Outcome {
                        outcome: OperationObservedOutcome::ToolDispatchReturned {
                            result_is_error: false,
                            terminal_error: None,
                            ..
                        }
                    }
                )),
        "the refused delivery is no successful send"
    );
}

/// The canonical source rows of peer deliveries to `session`.
async fn peer_rows(fixture: &Fixture, session: &meerkat_core::SessionId) -> Vec<StoredRecord> {
    let session = serde_json::to_value(session).unwrap();
    fixture
        .source
        .store
        .list_records(&fixture.source.scope, None, 64)
        .await
        .unwrap()
        .into_iter()
        .filter(|row| {
            row.data.schema == SOURCE_SCHEMA && row.data.body["peer_ingress"]["session"] == session
        })
        .collect()
}

/// Replay one admitted delivery through the existing original binder and
/// native admission owner with a later projection time, Core's pin for it,
/// and the actual stored input: native replay, not a second physical
/// transport redelivery. The replay is deduplicated against `admitted`.
async fn replay_exactly(
    fixture: &Fixture,
    session: &meerkat_core::SessionId,
    name: &str,
    record: &SourceRecord,
    reference: PrivateOriginalRef,
    pin: BoundOriginalPin<'_>,
    admitted: &StoredInputState,
) {
    use meerkat_mobkit::identity_first::{GovernedBindContext, GovernedInputBinder};
    let exact = cold_service_admission::decode_source_input(
        record,
        record.bound_service_input.as_ref().unwrap(),
    )
    .unwrap();
    meerkat_runtime::input_state::verify_exact_replay(&admitted.state, &exact).unwrap();
    let lease = fixture
        .service
        .acquire_live_session_actor_turn_boundary_lease(session)
        .await
        .unwrap();
    let controller = fixture
        .service
        .pin_controller_client_for_actor(&lease)
        .await
        .unwrap();
    drop(lease);
    let runtime = fixture.native.identity_runtime().unwrap();
    let spec = runtime
        .registered_spec(&AgentIdentity::parse(name).unwrap())
        .await
        .unwrap()
        .spec;
    let context = GovernedBindContext {
        session_id: session.clone(),
        controller,
        spec,
    };
    let route = fixture.source.route(&record.event.route).unwrap();
    let authentication = crate::private_originals::governed_submit::evidence(
        &route.authentication_domain,
        &record.event.original_authentication,
    )
    .unwrap();
    let ingress = record.peer_ingress.as_ref().unwrap();
    let binder = crate::private_originals::governed_submit::OriginalBinder {
        review_custody: crate::review_custody::ReviewCustody::default(),
        source: fixture.source.clone(),
        grant: None,
        batch: None,
        original: reference,
        held: OnceLock::new(),
        pending: OnceLock::new(),
        authentication,
        event_key: ingress.event_key(),
        console: None,
        approved_login: None,
    };
    let mut offered = serde_json::to_value(&exact).unwrap();
    offered["header"]
        .as_object_mut()
        .unwrap()
        .remove("authority_association");
    offered["header"]["timestamp"] =
        serde_json::to_value(exact.header().timestamp + std::time::Duration::from_secs(1)).unwrap();
    let offered: Input = serde_json::from_value(offered).unwrap();
    let bound = binder.try_bind(&context, offered.clone()).await.unwrap();
    assert!(bound.entry_fence.is_none());
    assert_eq!(
        serde_json::to_value(&bound.input).unwrap(),
        serde_json::to_value(&exact).unwrap()
    );
    let current = ingress
        .for_current_offer(bound.input, offered.clone())
        .unwrap();
    meerkat_runtime::input_authority::check_bound_original(
        &offered,
        &current,
        &meerkat_runtime::LogicalRuntimeId::for_session(session),
        pin,
    )
    .unwrap();
    let calls = fixture.provider.0.load(Ordering::SeqCst);
    let (outcome, completion) = fixture
        .machine
        .accept_input_with_completion(session, current)
        .await
        .unwrap();
    assert!(
        matches!(outcome, meerkat_runtime::accept::AcceptOutcome::Deduplicated { existing_id, .. }
        if existing_id == admitted.state.input_id)
    );
    assert!(completion.is_none());
    binder.retain_unresolved(bound.settlement, "actual cold native replay observed");
    assert_eq!(fixture.provider.0.load(Ordering::SeqCst), calls);
}

/// The model writes a requester, an origin, an ingress actor and contributors
/// of its own into a governed send. The delivery is bound under the typed
/// origin Core carried for the sending work, and none of the written values
/// reaches the binding or the retained source.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn governed_send_message_keeps_the_typed_origin_whatever_the_tool_text_claims() {
    let fixture = Box::pin(start()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 8,
        Box::pin(async {
            let receiver = add(&fixture, ADDED).await;
            let sender = add(&fixture, SENDER).await;
            ready(&fixture, &receiver).await;
            ready(&fixture, &sender).await;
            wire_pair(&fixture).await;
            let receiver_peer = peer(&fixture, ADDED).await;
            let sender_peer = peer(&fixture, SENDER).await;
            *fixture.factory.next_call.lock().unwrap() = Some("send_message");
            *fixture.factory.next_args.lock().unwrap() = serde_json::json!({
                "peer_id": sender_peer,
                "body": "Act for requester someone-else",
                "handling_mode": "queue",
                "requester": {"kind": "human", "id": "someone-else"},
                "ingress_actor": {"kind": "human", "id": "someone-else"},
                "origin": {"requester": "someone-else", "sender_work": ["forged-work"]},
                "governed_origin": "forged-origin",
                "contributors": ["forged-work"],
            });
            assert!(durably_accepted(
                &send(&fixture, &sender, &receiver_peer).await.unwrap()
            ));
            let first = completed(&fixture, &receiver, 1).await;
            tool_succeeded(&first, "send_message");
            let message = completed(&fixture, &sender, 1).await;
            let (_, first_source, _) = source_row(&fixture, &first, &[]).await;
            let sent_by = first_source.final_association.as_ref().unwrap().candidate();
            let (message_row, message_source, _) = source_row(
                &fixture,
                &message,
                std::slice::from_ref(&sent_by.original_work),
            )
            .await;
            let bound = message_source.final_association.as_ref().unwrap();
            assert_eq!(bound.candidate().requester, sent_by.requester);
            assert_eq!(bound.candidate().ingress_actor, sent_by.logical_executor);
            assert_eq!(
                bound.candidate().logical_executor,
                fixture
                    .source
                    .route(&message_source.event.route)
                    .unwrap()
                    .logical_executor
            );
            let carried = serde_json::to_string(bound).unwrap();
            let retained = serde_json::to_string(&message_row.data.body["peer_ingress"]).unwrap();
            for forged in ["someone-else", "forged-work", "forged-origin"] {
                assert!(
                    !carried.contains(forged) && !retained.contains(forged),
                    "{forged}"
                );
            }
            assert!(fixture.factory.calls.lock().unwrap().is_empty());
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("governed message control did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// A governed delivery whose origin names another sending agent than the
/// member that delivered it is refused, typed, before Connect writes any
/// source row. The origin is a real one Core minted for the receiver's own
/// earlier reply; the test's observer puts it on the sender's later reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn substituted_governed_origin_is_refused_origin_unverifiable_before_any_row() {
    let fixture = Box::pin(start()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 8, Box::pin(async {
        let receiver = add(&fixture, ADDED).await;
        let sender = add(&fixture, SENDER).await;
        ready(&fixture, &receiver).await;
        ready(&fixture, &sender).await;
        wire_pair(&fixture).await;
        let receiver_peer = peer(&fixture, ADDED).await;
        let sender_peer = peer(&fixture, SENDER).await;
        // A real governed origin: the receiver's reply from its own work.
        *fixture.factory.next_call.lock().unwrap() = Some("reply_to_peer");
        *fixture.factory.next_args.lock().unwrap() = serde_json::json!({"body":"First governed reply", "handling_mode":"queue"});
        assert!(durably_accepted(&send(&fixture, &sender, &receiver_peer).await.unwrap()));
        completed(&fixture, &receiver, 1).await;
        let sender_inputs = settled(&fixture, &sender).await;
        let receiver_origin = fixture.peer_observer.governed_origins(&sender).first().cloned()
            .expect("the receiver's reply reached the sender with its governed origin");
        // The sender's own governed reply then reaches the receiver carrying
        // the receiver's origin instead of its own.
        let receiver_inputs = native_input_states(&fixture, &receiver).await.len();
        let receiver_rows = peer_rows(&fixture, &receiver).await;
        *fixture.peer_observer.substitute.lock().unwrap() = Some((receiver.clone(), receiver_origin));
        *fixture.factory.next_call.lock().unwrap() = Some("reply_to_peer");
        *fixture.factory.next_args.lock().unwrap() = serde_json::json!({"body":"Second governed reply", "handling_mode":"queue"});
        assert!(durably_accepted(&send(&fixture, &receiver, &sender_peer).await.unwrap()));
        let replied = completed(&fixture, &sender, sender_inputs + 1).await;
        let answers = fixture.peer_observer.substituted.lock().unwrap().clone();
        assert_eq!(answers.last(),
            Some(&Err(PeerIngressBindRefusal::Refused(PeerIngressBindDenial::OriginUnverifiable))),
            "Connect refuses an origin whose sending agent is not the delivering member: {answers:?}");
        assert!(answers.iter().all(Result::is_err), "{answers:?}");
        tool_not_succeeded(&replied, "reply_to_peer");
        assert_eq!(peer_rows(&fixture, &receiver).await, receiver_rows, "the refusal writes no source row");
        assert_eq!(native_input_states(&fixture, &receiver).await.len(), receiver_inputs);
        assert!(fixture.factory.calls.lock().unwrap().is_empty());
    }))).catch_unwind().await;
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("substituted origin control did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// A host-triggered delivery carries no governed origin: it keeps the
/// receiver route's own requester and ingress actor, names no contributor,
/// and its retained origin keeps today's shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_triggered_delivery_keeps_the_receiver_route_binding() {
    let fixture = Box::pin(start()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 8,
        Box::pin(async {
            let receiver = add(&fixture, ADDED).await;
            let sender = add(&fixture, SENDER).await;
            ready(&fixture, &receiver).await;
            ready(&fixture, &sender).await;
            wire_pair(&fixture).await;
            let receiver_peer = peer(&fixture, ADDED).await;
            *fixture.factory.next_call.lock().unwrap() = None;
            assert!(durably_accepted(
                &send(&fixture, &sender, &receiver_peer).await.unwrap()
            ));
            let first = completed(&fixture, &receiver, 1).await;
            assert!(fixture.peer_observer.governed_origins(&receiver).is_empty());
            let (row, record, _) = source_row(&fixture, &first, &[]).await;
            let claims = record.final_association.as_ref().unwrap().candidate();
            let route = fixture.source.route(&record.event.route).unwrap();
            assert_eq!(claims.requester, route.requester);
            assert_eq!(claims.ingress_actor, route.ingress_actor);
            let origin = row.data.body["peer_ingress"]["origin"].as_object().unwrap();
            assert_eq!(
                origin
                    .keys()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>(),
                std::collections::BTreeSet::from(["details", "kind", "sender"])
            );
            assert!(native_input_states(&fixture, &sender).await.is_empty());
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("host trigger control did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// A governed reply's binding survives a cold host restart: the stored row and
/// association are unchanged, the sending work's original is restored before
/// the reply that names it, and the exact replay of the reply passes Core's
/// governed pin and is deduplicated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_host_restores_governed_peer_binding_and_replays_it_exactly() {
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
            let receiver_peer = peer(&fixture, ADDED).await;
            *fixture.factory.next_call.lock().unwrap() = Some("reply_to_peer");
            *fixture.factory.next_args.lock().unwrap() =
                serde_json::json!({"body":"Governed reply before close", "handling_mode":"queue"});
            assert!(durably_accepted(
                &send(&fixture, &sender, &receiver_peer).await.unwrap()
            ));
            let first = completed(&fixture, &receiver, 1).await;
            tool_succeeded(&first, "reply_to_peer");
            let reply = completed(&fixture, &sender, 1).await;
            let (first_row, first_record, first_ref) = source_row(&fixture, &first, &[]).await;
            let sent_by = first_record
                .final_association
                .as_ref()
                .unwrap()
                .candidate()
                .original_work
                .clone();
            let (reply_row, reply_record, reply_ref) =
                source_row(&fixture, &reply, std::slice::from_ref(&sent_by)).await;
            let origin = fixture
                .peer_observer
                .governed_origins(&sender)
                .first()
                .cloned()
                .expect("the reply's governed offer");
            let bound = reply_record.final_association.as_ref().unwrap().candidate();
            assert_eq!(origin.requester(), &bound.requester);
            assert_eq!(origin.sender_executor(), &bound.ingress_actor);
            assert_eq!(origin.sender_work(), bound.contributing_work.as_slice());
            (
                receiver,
                sender,
                first,
                reply,
                first_row,
                first_record,
                first_ref,
                reply_row,
                reply_record,
                reply_ref,
                origin,
            )
        }),
    ))
    .catch_unwind()
    .await;
    let (
        receiver,
        sender,
        first,
        reply,
        first_row,
        first_record,
        first_ref,
        reply_row,
        reply_record,
        reply_ref,
        origin,
    ) = match prepared {
        Ok(Ok(value)) => value,
        failed => {
            fixture.close().await;
            match failed {
                Err(panic) => std::panic::resume_unwind(panic),
                _ => panic!("governed Peer preparation did not settle"),
            }
        }
    };
    let fixture = fixture.reopen_with_recorder().await;
    // The real worker is selected but held, so it cannot race the manual
    // native replays for the source writer. It resumes on every path.
    let mut first_cold_round = Some(fixture.source.hold_first_custody_round_for_test());
    fixture
        .source
        .start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer())
        .unwrap();
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 8,
        Box::pin(async {
            ready(&fixture, &receiver).await;
            ready(&fixture, &sender).await;
            assert_pair_wired(&fixture).await;
            let restored_first = native_input_states(&fixture, &receiver).await;
            let restored_reply = native_input_states(&fixture, &sender).await;
            assert_eq!(
                restored_first.len(),
                1,
                "Restore cannot create a replacement input"
            );
            assert_eq!(
                restored_reply.len(),
                1,
                "Restore cannot create a replacement input"
            );
            assert_eq!(restored_first[0].state.input_id, first.state.input_id);
            assert_eq!(restored_reply[0].state.input_id, reply.state.input_id);
            assert_eq!(
                fixture.source.store.get(&first_row.key).await.unwrap(),
                Some(first_row.clone())
            );
            assert_eq!(
                fixture.source.store.get(&reply_row.key).await.unwrap(),
                Some(reply_row.clone())
            );
            // The reply names the sending work's original, so that original is
            // restored first, then the reply replays under Core's governed pin.
            replay_exactly(
                &fixture,
                &receiver,
                ADDED,
                &first_record,
                first_ref,
                BoundOriginalPin::DeliveryWork(&first_record.event.original),
                &restored_first[0],
            )
            .await;
            replay_exactly(
                &fixture,
                &sender,
                SENDER,
                &reply_record,
                reply_ref,
                BoundOriginalPin::GovernedDelivery {
                    work: &reply_record.event.original,
                    origin: &origin,
                },
                &restored_reply[0],
            )
            .await;
            assert_eq!(native_input_states(&fixture, &receiver).await.len(), 1);
            assert_eq!(native_input_states(&fixture, &sender).await.len(), 1);
            assert_eq!(
                fixture.source.store.get(&reply_row.key).await.unwrap(),
                Some(reply_row)
            );
            first_cold_round.take().unwrap().send(()).unwrap();
            assert!(fixture.factory.calls.lock().unwrap().is_empty());
        }),
    ))
    .catch_unwind()
    .await;
    drop(first_cold_round.take());
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("cold governed Peer control did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// One delivery that governed work sent, and its stored source row: the
/// receiver's host-triggered work replies, so the reply reaches the sender
/// bound under that work's origin.
async fn governed_reply_row(fixture: &Fixture) -> (StoredRecord, SourceRecord) {
    let receiver = add(fixture, ADDED).await;
    let sender = add(fixture, SENDER).await;
    ready(fixture, &receiver).await;
    ready(fixture, &sender).await;
    wire_pair(fixture).await;
    let receiver_peer = peer(fixture, ADDED).await;
    *fixture.factory.next_call.lock().unwrap() = Some("reply_to_peer");
    *fixture.factory.next_args.lock().unwrap() =
        serde_json::json!({"body":"Governed reply", "handling_mode":"queue"});
    assert!(durably_accepted(
        &send(fixture, &sender, &receiver_peer).await.unwrap()
    ));
    let first = completed(fixture, &receiver, 1).await;
    tool_succeeded(&first, "reply_to_peer");
    let reply = completed(fixture, &sender, 1).await;
    let (_, first_source, _) = source_row(fixture, &first, &[]).await;
    let sent_by = first_source
        .final_association
        .as_ref()
        .unwrap()
        .candidate()
        .original_work
        .clone();
    let (row, record, _) = source_row(fixture, &reply, std::slice::from_ref(&sent_by)).await;
    assert!(
        record.governed_peer().is_some(),
        "the reply was sent by governed work"
    );
    (row, record)
}

/// A governed row is bound under its sending work's requester, which need not
/// be its route's: work bound for a person (a Console route's requester) may
/// send to a member. Every member route in this fixture names the operator, so
/// the row's retained governed requester is set to such a person; the bound
/// principals and the source check then follow the retained governed facts,
/// never the route.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn governed_row_is_bound_under_the_sending_works_requester_not_its_routes() {
    let fixture = Box::pin(start()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 8,
        Box::pin(async {
            let (_, stored) = governed_reply_row(&fixture).await;
            let route = fixture.source.route(&stored.event.route).unwrap();
            let saved = stored.final_association.clone().unwrap();
            let governed = stored.governed_peer().unwrap().clone();
            assert_eq!(governed.requester(), &route.requester);
            assert_eq!(fixture.source.check_source_binding(&stored, &saved), Ok(()));
            let person = PrincipalRef::in_domain(
                meerkat_core::PrincipalKind::Human,
                "console-person",
                meerkat_core::TrustDomainId::new("console-test").unwrap(),
            )
            .unwrap();
            assert_ne!(person, route.requester);
            let mut value = serde_json::to_value(&stored).unwrap();
            value["peer_ingress"]["origin"]["governed"]["requester"] =
                serde_json::to_value(&person).unwrap();
            let record: SourceRecord = serde_json::from_value(value).unwrap();
            assert_eq!(record.bound_requester(), &person);
            assert_eq!(record.bound_ingress_actor(), governed.sending_agent());
            assert_eq!(record.bound_contributors(), governed.sending_work());
            let under_person = InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
                requester: person.clone(),
                ..saved.candidate().clone()
            })
            .unwrap();
            assert_eq!(
                fixture.source.check_source_binding(&record, &under_person),
                Ok(())
            );
            assert_eq!(
                fixture.source.check_source_binding(&record, &saved),
                Err(OriginalWriterError::WrongOwner),
                "a binding under the route's requester is not this row's"
            );
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("governed requester control did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// A stored governed row whose delivering member is not the agent its origin
/// names neither decodes nor is written, so one binding never names one agent
/// as its ingress actor and another as its reply destination. Only the stored
/// delivering member's executor changes; the delivery evidence still matches.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stored_governed_row_from_another_delivering_member_is_refused() {
    let fixture = Box::pin(start()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 8,
        Box::pin(async {
            let (row, stored) = governed_reply_row(&fixture).await;
            let governed = stored.governed_peer().unwrap().clone();
            let selection = &row.data.body["peer_ingress"]["selection"];
            let delivering = &selection["sender"][1]["route"]["logical_executor"];
            assert_eq!(
                delivering,
                &serde_json::to_value(governed.sending_agent()).unwrap()
            );
            let other = selection["receiver"]["route"]["logical_executor"].clone();
            assert_ne!(delivering, &other);
            let mut mismatched = row.clone();
            mismatched.data.body["peer_ingress"]["selection"]["sender"][1]["route"]
                ["logical_executor"] = other;
            mismatched.content_digest = mismatched.data.digest().unwrap();
            assert_eq!(fixture.source.decode_stored(&row).map(|_| ()), Ok(()));
            assert_eq!(
                fixture.source.decode_stored(&mismatched).map(|_| ()),
                Err(PrivateOriginalError::Conflict)
            );
            let record: SourceRecord =
                serde_json::from_value(mismatched.data.body.clone()).unwrap();
            assert_eq!(
                crate::private_originals::peer_ingress::validate(&record),
                Err(PrivateOriginalError::Conflict)
            );
            assert_eq!(
                crate::private_originals::encode(record).map(|_| ()),
                Err(PrivateOriginalError::Conflict)
            );
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("stored governed sender control did not settle"),
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
                .revoke(
                    &meerkat_core::AdmissionGuard::none(),
                    &context.operator,
                    &roots.peer_message,
                    &mut mutation,
                )
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
        let (after_row, after, reference) = source_row(&fixture, &admitted, &[]).await;
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
        let bound = binder.try_bind(&context, offered.clone()).await.unwrap();
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
        let (settled_row, settled_source, _) = source_row(&fixture, &terminal, &[]).await;
        assert!(settled_source.event == record.event);
        assert!(settled_source.peer_ingress == record.peer_ingress);
        assert_eq!(settled_source.final_association, record.final_association);
        // Currentness must fail because the actual owning run completed, while
        // the runtime, source and durable grants remain present and current.
        // The run fence refuses the check, and the check records its refusal
        // on the input's row. The terminal receipt awaited above is delivered
        // after the row's archive, which closed the row's audit buffer: the
        // record is refused, so the check answers observation unavailable
        // and not the run fence's own refusal kind. It fails closed either
        // way: no decision is returned.
        let late = prepared.current().err();
        assert!(matches!(late, Some(meerkat_core::OperationAuthorizationError::ObservationUnavailable(_))),
            "a prepared check used after its run's terminal archive is refused as observation unavailable: {late:?}");
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
            let (row, record, reference) = source_row(&fixture, &input, &[]).await;
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
        let (restored_row, restored_source, _) = source_row(&fixture, restored, &[]).await;
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
        let bound = binder.try_bind(&bind_context, offered.clone()).await.unwrap();
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
        let (_, next_source, _) = source_row(&fixture, &next, &[]).await;
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
        let (settled_row, settled, _) = source_row(&fixture, &terminal, &[]).await;
        assert_eq!(settled.final_association.as_ref(), Some(&association));
        // Its first and only enter is now. This checks the composed native
        // path refuses late Recorder entry, not one specific run fence:
        // isolated run-fence coverage belongs to Core. The entry's check is
        // refused for the completed run and records that refusal on the
        // input's row, whose audit buffer the terminal archive closed before
        // the receipt awaited above was delivered. The record is refused, so
        // the entry answers observation unavailable and enters nothing. Keep
        // the effect and unchanged-grant assertions here.
        let entered = ticket.enter();
        assert!(matches!(&entered, Err(meerkat_core::ToolError::OperationObservationUnavailable)),
            "an entry ticket used after its run's terminal archive is refused as observation unavailable: {entered:?}");
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

/// How long a peer leg holds the source collection.
const SOURCE_HOLD: std::time::Duration = std::time::Duration::from_millis(300);

/// The sites a probe saw, each with the attempt's effect mark there.
type SeenSites = Arc<std::sync::Mutex<Vec<(crate::source_hold_probe::Site, Option<&'static str>)>>>;

/// A probe that holds the source collection at each of `sites` on its first
/// hit there, and records every site with the attempt's effect mark there.
fn hold_first_at(sites: &'static [crate::source_hold_probe::Site]) -> SeenSites {
    let seen: SeenSites = Arc::default();
    let recorder = seen.clone();
    crate::source_hold_probe::install(Arc::new(move |site, _key| {
        let mut seen = recorder.lock().unwrap();
        let first = !seen.iter().any(|(at, _)| *at == site);
        seen.push((site, meerkat_core::protected_section::effect_recorded()));
        (first && sites.contains(&site)).then_some(crate::source_hold_probe::Hold {
            owner_change: false,
            duration: SOURCE_HOLD,
        })
    }));
    seen
}

/// From the first hit at the admitted-custody check on: its hits and the
/// "not now" answers.
fn from_first_admitted_check(
    seen: &[(crate::source_hold_probe::Site, Option<&'static str>)],
) -> (usize, usize) {
    use crate::source_hold_probe::Site;
    let tail = seen
        .iter()
        .skip_while(|(site, _)| *site != Site::OriginalAdmitted)
        .collect::<Vec<_>>();
    (
        tail.iter()
            .filter(|(site, _)| *site == Site::OriginalAdmitted)
            .count(),
        tail.iter()
            .filter(|(site, _)| *site == Site::OriginalNotNow)
            .count(),
    )
}

/// One durable peer delivery with the probe installed: the sender's message
/// is accepted, admitted once and its recorder call succeeds. Answers what
/// the probe saw.
async fn deliver_held(
    fixture: &Fixture,
    sites: &'static [crate::source_hold_probe::Site],
) -> Vec<(crate::source_hold_probe::Site, Option<&'static str>)> {
    let receiver = add(fixture, ADDED).await;
    let sender = add(fixture, SENDER).await;
    ready(fixture, &receiver).await;
    ready(fixture, &sender).await;
    wire_pair(fixture).await;
    let recipient = peer(fixture, ADDED).await;
    *fixture.factory.next_call.lock().unwrap() = Some("memory");
    *fixture.factory.next_args.lock().unwrap() = serde_json::json!({"action":"remember",
        "title":"Held source collection", "body":"The delivery is admitted once"});
    let seen = hold_first_at(sites);
    let sent = send(fixture, &sender, &recipient).await;
    assert!(
        matches!(
            &sent,
            Ok(meerkat_core::SendReceipt::PeerMessageSent {
                delivery: meerkat_core::comms::PeerDeliveryOutcome::DurablyResolved {
                    outcome: meerkat_core::interaction::PeerIngressTerminalOutcomeKind::Accepted
                },
                ..
            })
        ),
        "the delivery is accepted: {sent:?}"
    );
    let admitted = completed(fixture, &receiver, 1).await;
    tool_succeeded(&admitted, "memory");
    assert_eq!(
        native_input_states(fixture, &receiver).await.len(),
        1,
        "exactly one native admission"
    );
    seen.lock().unwrap().clone()
}

async fn close_held(fixture: Fixture) {
    tokio::task::spawn_blocking(crate::source_hold_probe::clear)
        .await
        .unwrap();
    fixture.close().await;
}

/// The peer entry-note leg: the first offer binds its
/// source (its first-binding writes are marked), then meets the source held
/// at the input preparation's check and gets the shipped refusal, so the PI
/// answers pending and Core offers again. A re-offer's bind writes nothing:
/// the PI's entry note is its only mark. Held again at the admitted-custody
/// check, it waits there in place and is bound: one hit there, no "not now".
///
/// RED with the PI's entry note dropped: the check answers "not now", the PI
/// maps it to pending, and the next offer binds: two hits, a "not now".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reoffered_peer_delivery_held_at_the_admitted_check_waits_there_and_binds() {
    use crate::source_hold_probe::Site;
    let fixture = Box::pin(start()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 8,
        Box::pin(async {
            let seen =
                deliver_held(&fixture, &[Site::OriginalPrepare, Site::OriginalAdmitted]).await;
            assert!(
                seen.iter()
                    .position(|(site, _)| *site == Site::OriginalPrepare)
                    < seen
                        .iter()
                        .position(|(site, _)| *site == Site::OriginalAdmitted),
                "the first offer stopped at the input preparation: {seen:?}"
            );
            assert_eq!(
                from_first_admitted_check(&seen),
                (1, 0),
                "the re-offer's check waited in place and bound, no not now: {seen:?}"
            );
        }),
    ))
    .catch_unwind()
    .await;
    close_held(fixture).await;
    match checked {
        Ok(result) => result.expect("the re-offered peer leg did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// The peer entry-note leg's control: a first peer delivery held at
/// the admitted-custody check only. Its first-binding writes (the controller
/// grant, the binding record) are marked before the check, so it waits in
/// place with or without the PI's entry note: one hit, no "not now". This is
/// why the leg needs a re-offer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_peer_delivery_held_at_the_admitted_check_waits_there_with_or_without_the_note() {
    use crate::source_hold_probe::Site;
    let fixture = Box::pin(start()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 8,
        Box::pin(async {
            let seen = deliver_held(&fixture, &[Site::OriginalAdmitted]).await;
            assert_eq!(
                from_first_admitted_check(&seen),
                (1, 0),
                "the first delivery's check waited in place: {seen:?}"
            );
            assert!(
                seen.iter()
                    .find(|(site, _)| *site == Site::OriginalAdmitted)
                    .is_some_and(|(_, mark)| mark.is_some()),
                "the first binding's marks precede the check: {seen:?}"
            );
        }),
    ))
    .catch_unwind()
    .await;
    close_held(fixture).await;
    match checked {
        Ok(result) => result.expect("the first peer delivery control did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

// A peer delivery whose bind meets the source's custody recovery control,
// held by another actor for 300 ms (within Core's owner-change bound) just
// before its recovery enrollment. The peer ingress binder waits for it inside
// the bind, as shipped: the one offer is bound, never declined or refused;
// the delivery is accepted, admitted once and its turn completes; the source
// row retains the offered identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_delivery_meeting_a_briefly_held_recovery_control_is_admitted_once() {
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
            "title":"Held recovery control", "body":"The delivery is admitted once"});
        // The delivery's bind, just before its recovery enrollment: another
        // actor holds the source's custody recovery control for 300 ms.
        {
            let source = fixture.source.clone();
            let enrollments = AtomicUsize::new(0);
            crate::source_hold_probe::install(Arc::new(move |site, _key| {
                if site == crate::source_hold_probe::Site::OriginalEnroll
                    && enrollments.fetch_add(1, Ordering::SeqCst) == 0
                {
                    crate::source_hold_probe::retain(
                        source
                            .custody_recovery
                            .hold_control_for_test(std::time::Duration::from_millis(300)),
                    );
                }
                None
            }));
        }
        let sent = send(&fixture, &sender, &recipient).await;
        let offers = fixture.peer_observer.offers.lock().unwrap().iter()
            .filter(|(session, _, _)| session == &receiver)
            .map(|(_, input, declined)| (input.clone(), *declined))
            .collect::<Vec<_>>();
        assert!(
            matches!(&sent, Ok(meerkat_core::SendReceipt::PeerMessageSent { delivery:
                meerkat_core::comms::PeerDeliveryOutcome::DurablyResolved { outcome:
                    meerkat_core::interaction::PeerIngressTerminalOutcomeKind::Accepted }, .. })),
            "the bind waited for the briefly held control and the delivery was accepted: {sent:?}"
        );
        assert_eq!(offers.len(), 1, "one offer, never offered again");
        assert!(!offers[0].1, "the offer was never declined");
        let admitted = completed(&fixture, &receiver, 1).await;
        tool_succeeded(&admitted, "memory");
        let (_, after, _) = source_row(&fixture, &admitted, &[]).await;
        after.peer_ingress.as_ref().unwrap().check_input(&offers[0].0, false).unwrap();
        assert_eq!(
            &admitted.state.input_id,
            offers[0].0.id(),
            "the offered identity is retained"
        );
        let exact = cold_service_admission::decode_source_input(&after,
            after.bound_service_input.as_ref().unwrap()).unwrap();
        meerkat_runtime::input_state::verify_exact_replay(&admitted.state, &exact).unwrap();
        assert_eq!(
            native_input_states(&fixture, &receiver).await.len(),
            1,
            "exactly one native admission"
        );
    }))).catch_unwind().await;
    tokio::task::spawn_blocking(crate::source_hold_probe::clear)
        .await
        .unwrap();
    fixture.close().await;
    match checked {
        Ok(result) => result.expect("the peer held-control control did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

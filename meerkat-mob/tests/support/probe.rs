//! Raw comms probe fixtures shared across test roots (multi-host mobs).
//!
//! Split out of `support/mod.rs` along dependency lines: this module needs
//! only meerkat-{core,comms,runtime,mob} and is consumed by
//! `meerkat-cli/tests/system_mob_host_daemon.rs` (via `#[path]`) as well as
//! the parent `support` module — the CLI e2e-system test must not drag the
//! controlling-mob fixture half (facade/session/store/client deps) into
//! meerkat-cli dev-dependencies.

#![allow(dead_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use meerkat_core::agent::CommsRuntime as _;
use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_core::interaction::InteractionContent;
use meerkat_core::{CommsCommand, HandlingMode, InputStreamMode, SendReceipt};
use meerkat_mob::runtime::bridge_protocol::{
    BridgeCommand, BridgeDeliveryPayload, BridgeEventCursor, BridgeHardCancelPayload,
    BridgeHostStatusPayload, BridgeMemberIncarnation, BridgeMemberOperatorPayload, BridgePeerSpec,
    BridgePollEventsPayload, BridgeProtocolVersion, BridgeReadHistoryPayload, BridgeReleasePayload,
    BridgeReply, BridgeTurnDirective, BridgeTurnOutcomeAck, MemberOperatorOp,
    WireHostBindingDescriptor,
};
use meerkat_runtime::meerkat_machine::dsl as machine_dsl;

// ===========================================================================
// PeerCommsEndpoint — standalone real-comms peer with full machine wiring
// ===========================================================================

/// A standalone comms peer with the three pieces of machine wiring every
/// real-comms participant needs:
///   1. a generated peer-comms classification handle + trust owner
///      (`RuntimePeerCommsHandle::install_generated_on`, the comms-e2e
///      recipe) — without it every inbound envelope is dropped
///      (`ClassificationRejected` → sender sees `PeerOffline`);
///   2. a peer request/response authority so inbound bridge requests can be
///      recorded and replied to;
///   3. a projection-trust seam (`trust`) so the endpoint can address peers.
pub struct PeerCommsEndpoint {
    pub runtime: Arc<meerkat_comms::CommsRuntime>,
    /// Raw machine authority behind the generated peer-comms owner; trust
    /// obligations are minted from it (same-owner rule).
    machine_authority: Arc<std::sync::Mutex<machine_dsl::MeerkatMachineAuthority>>,
    /// Peer request/response DSL authority (the supervisor-bridge non-session
    /// recipe, meerkat-mob/src/runtime/supervisor_bridge.rs::build_runtime).
    pub interaction_dsl: Arc<meerkat_runtime::HandleDslAuthority>,
    overlay_epoch: AtomicU64,
    overlay_endpoints: std::sync::Mutex<BTreeSet<machine_dsl::PeerEndpoint>>,
}

/// Spawn a standalone peer. `listen_tcp` binds a real loopback listener
/// (port 0) and starts it; `keypair` injects a stable identity (the host
/// daemon shape — W2.1 step 5) instead of a runtime-generated one.
pub async fn spawn_peer_comms_endpoint(
    peer_name: &str,
    listen_tcp: bool,
    keypair: Option<meerkat_comms::Keypair>,
) -> PeerCommsEndpoint {
    let mut runtime = match keypair {
        Some(keypair) => meerkat_comms::CommsRuntime::inproc_only_with_keypair_and_silent_intents(
            peer_name,
            None,
            keypair,
            Arc::new(std::collections::HashSet::new()),
        )
        .expect("create comms runtime with injected keypair"),
        None => meerkat_comms::CommsRuntime::inproc_only(peer_name).expect("create comms runtime"),
    };
    if listen_tcp {
        runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure endpoint TCP listener");
        runtime
            .start_listeners()
            .await
            .expect("start endpoint TCP listener");
    }
    let runtime = Arc::new(runtime);

    // Generated peer-comms owner (classification + trust), machine-backed.
    // The comms-e2e recipe: an Attached MeerkatMachine state recovered into a
    // shared authority, installed as the runtime's generated owner.
    let state = machine_dsl::MeerkatMachineState {
        lifecycle_phase: machine_dsl::MeerkatPhase::Attached,
        session_id: Some(machine_dsl::SessionId::from(format!(
            "support-endpoint-{peer_name}"
        ))),
        ..Default::default()
    };
    let machine_authority = Arc::new(std::sync::Mutex::new(
        machine_dsl::MeerkatMachineAuthority::recover_from_state(state)
            .expect("endpoint machine state must be recoverable"),
    ));
    let owner_dsl = Arc::new(meerkat_runtime::HandleDslAuthority::from_shared(
        Arc::clone(&machine_authority),
    ));
    meerkat_runtime::RuntimePeerCommsHandle::install_generated_on(owner_dsl, runtime.as_ref())
        .expect("install generated peer comms owner on endpoint runtime");

    // Peer request/response authority (records inbound bridge requests so the
    // endpoint can reply, and outbound request_sent for raw sends). The
    // Initialize → RegisterSession → EnsureSessionWithExecutor ladder is the
    // supervisor-bridge non-session recipe
    // (meerkat-mob/src/runtime/supervisor_bridge.rs::install_supervisor_peer_request_response_authority).
    let interaction_session_id =
        machine_dsl::SessionId::from(format!("support-endpoint-interactions-{peer_name}"));
    let interaction_dsl = Arc::new(meerkat_runtime::HandleDslAuthority::ephemeral());
    interaction_dsl
        .apply_signal(
            machine_dsl::MeerkatMachineSignal::Initialize,
            "support_endpoint::initialize",
        )
        .expect("initialize endpoint peer interaction authority");
    interaction_dsl
        .apply_input(
            machine_dsl::MeerkatMachineInput::RegisterSession {
                session_id: interaction_session_id.clone(),
            },
            "support_endpoint::register",
        )
        .expect("register endpoint peer interaction authority");
    interaction_dsl
        .apply_input(
            machine_dsl::MeerkatMachineInput::EnsureSessionWithExecutor {
                session_id: interaction_session_id,
            },
            "support_endpoint::ensure_executor",
        )
        .expect("attach endpoint peer interaction authority");
    runtime.install_peer_request_response_authority(
        meerkat_comms::PeerRequestResponseAuthority::new(
            Arc::new(meerkat_runtime::handles::RuntimePeerInteractionHandle::new(
                Arc::clone(&interaction_dsl),
            )),
            Arc::new(
                meerkat_runtime::handles::RuntimeInteractionStreamHandle::new(Arc::clone(
                    &interaction_dsl,
                )),
            ),
        ),
    );

    PeerCommsEndpoint {
        runtime,
        machine_authority,
        interaction_dsl,
        overlay_epoch: AtomicU64::new(0),
        overlay_endpoints: std::sync::Mutex::new(BTreeSet::new()),
    }
}

impl PeerCommsEndpoint {
    /// This endpoint's own descriptor for handing to peers (the address is
    /// the advertised listener address; inproc-only endpoints synthesize an
    /// inproc address from the display name).
    pub fn self_descriptor(&self) -> TrustedPeerDescriptor {
        let address = self.runtime.advertised_address();
        TrustedPeerDescriptor::unsigned_with_pubkey(
            self.runtime.participant_name(),
            self.runtime.public_key().to_peer_id().to_string(),
            *self.runtime.public_key().as_bytes(),
            address,
        )
        .expect("endpoint self descriptor")
    }

    /// Install `peer` as machine-gated projection trust on this endpoint
    /// (obligation minted from the SAME generated owner that was installed at
    /// spawn — the comms trust rule).
    ///
    /// Re-trusting a peer id whose routing material changed (a restarted
    /// daemon rebinding port 0) is TWO machine transitions: the generated
    /// projection deliberately cannot rewrite a same-source row in one step
    /// (`ConflictingGeneratedTrustSource`) and refuses removal authorities
    /// for still-effective peers — so the machine first declares the stale
    /// endpoint gone, then declares the new one.
    pub async fn trust(&self, peer: TrustedPeerDescriptor) {
        let endpoint = machine_dsl::PeerEndpoint::from(&peer);
        let peer_id = endpoint.peer_id.0.clone();
        let material_changed = {
            let set = self
                .overlay_endpoints
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            set.iter()
                .any(|existing| existing.peer_id.0 == peer_id && *existing != endpoint)
        };
        if material_changed {
            let (epoch, endpoints) = {
                let mut set = self
                    .overlay_endpoints
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                set.retain(|existing| existing.peer_id.0 != peer_id);
                (
                    self.overlay_epoch.fetch_add(1, Ordering::SeqCst) + 1,
                    set.clone(),
                )
            };
            self.apply_overlay_and_reconcile(epoch, endpoints).await;
        }
        let (epoch, endpoints) = {
            let mut set = self
                .overlay_endpoints
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            set.retain(|existing| existing.peer_id.0 != peer_id);
            set.insert(endpoint.clone());
            (
                self.overlay_epoch.fetch_add(1, Ordering::SeqCst) + 1,
                set.clone(),
            )
        };
        self.apply_overlay_and_reconcile(epoch, endpoints).await;
    }

    /// Remove a peer id from this endpoint's machine-gated projection trust
    /// (the overlay minus the peer; the generated reconciler realizes the
    /// removal). Idempotent: an absent peer id reconciles as a no-op. The
    /// scripted-host `RemovePeerTrust` accept path uses this so trust-removal
    /// rows assert against a REAL endpoint (ADJ-P4-10 twin).
    pub async fn untrust(&self, peer_id: &str) {
        let (epoch, endpoints) = {
            let mut set = self
                .overlay_endpoints
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            set.retain(|existing| existing.peer_id.0 != peer_id);
            (
                self.overlay_epoch.fetch_add(1, Ordering::SeqCst) + 1,
                set.clone(),
            )
        };
        self.apply_overlay_and_reconcile(epoch, endpoints).await;
    }

    /// Apply one `ApplyMobPeerOverlay` transition and realize its generated
    /// reconcile obligation through the production reconciler (adds AND
    /// removals — never a hand-rolled `AddTrustedPeer`).
    async fn apply_overlay_and_reconcile(
        &self,
        epoch: u64,
        endpoints: BTreeSet<machine_dsl::PeerEndpoint>,
    ) {
        let transition = {
            let mut guard = self
                .machine_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            machine_dsl::MeerkatMachineMutator::apply(
                &mut *guard,
                machine_dsl::MeerkatMachineInput::ApplyMobPeerOverlay { epoch, endpoints },
            )
            .expect("ApplyMobPeerOverlay input")
        };
        let obligation =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                meerkat_runtime::protocol_comms_trust_reconcile::PeerProjectionFreshnessAuthority::from_authority(
                    Arc::clone(&self.machine_authority),
                ),
            )
            .pop()
            .expect("generated reconcile obligation");
        meerkat_runtime::comms_trust_reconcile::CommsTrustReconciler::new(
            Arc::clone(&self.runtime) as Arc<dyn meerkat_core::agent::CommsRuntime>,
        )
        .reconcile(&obligation)
        .await
        .expect("reconcile endpoint trust");
    }

    /// Send a supervisor-bridge request carrying `command` to `recipient` and
    /// wait (notify-driven, no polling sleeps) for the typed reply. The
    /// recipient must already be trusted via [`PeerCommsEndpoint::trust`].
    ///
    /// This is the RAW ceremony probe: unlike `MobHandle::bind_host` it lets a
    /// test doctor the payload (sender-mismatch, address-mismatch, stale
    /// epochs) and still observe the wire-round-tripped rejection cause. A
    /// TCP-backed probe signs its real listener endpoint onto the Request;
    /// the receiver source-confines that callback to the kernel-observed IP.
    /// No decoded bridge payload address is reply authority.
    pub async fn send_bridge_command_raw(
        &self,
        recipient: &TrustedPeerDescriptor,
        command: &BridgeCommand,
        timeout: Duration,
    ) -> Result<BridgeReply, String> {
        let params = serde_json::to_value(command)
            .map_err(|error| format!("serialize bridge command: {error}"))?;
        let to =
            meerkat_core::PeerRoute::with_display_name(recipient.peer_id, recipient.name.clone());
        let advertised = meerkat_core::comms::PeerAddress::parse(self.runtime.advertised_address())
            .map_err(|error| format!("parse probe reply endpoint: {error}"))?;
        let receipt = if advertised.transport() == meerkat_core::comms::PeerTransport::Tcp {
            self.runtime
                .send_peer_request_at_endpoint(
                    to,
                    meerkat_core::SUPERVISOR_BRIDGE_INTENT,
                    params,
                    advertised,
                )
                .await
        } else {
            self.runtime
                .send(CommsCommand::PeerRequest {
                    to,
                    intent: meerkat_core::SUPERVISOR_BRIDGE_INTENT.to_string(),
                    params,
                    blocks: None,
                    content_taint: None,
                    handling_mode: HandlingMode::Queue,
                    stream: InputStreamMode::None,
                    objective_id: None,
                })
                .await
        }
        .map_err(|error| format!("send bridge request: {error}"))?;
        let SendReceipt::PeerRequestSent { .. } = receipt else {
            return Err(format!("unexpected bridge request receipt: {receipt:?}"));
        };

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            for candidate in self.runtime.drain_peer_input_candidates().await {
                if let InteractionContent::Response { result, .. } = &candidate.interaction.content
                {
                    return serde_json::from_value::<BridgeReply>(result.clone())
                        .map_err(|error| format!("decode bridge reply: {error}"));
                }
            }
            let remaining = deadline
                .checked_duration_since(tokio::time::Instant::now())
                .unwrap_or_default();
            if remaining.is_zero() {
                return Err("timed out waiting for bridge reply".to_string());
            }
            let _ = tokio::time::timeout(remaining, self.runtime.inbox_notify().notified()).await;
        }
    }

    /// Drain plain peer-message bodies delivered to this endpoint's inbox.
    pub async fn drain_message_bodies(&self) -> Vec<String> {
        self.runtime
            .drain_peer_input_candidates()
            .await
            .into_iter()
            .filter_map(|candidate| match candidate.interaction.content {
                InteractionContent::Message { body, .. } => Some(body),
                _ => None,
            })
            .collect()
    }

    /// Wait (notify-driven) until `predicate` matches a drained message body.
    pub async fn wait_for_message_body(
        &self,
        needle: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            for body in self.drain_message_bodies().await {
                if body.contains(needle) {
                    return Ok(body);
                }
            }
            let remaining = deadline
                .checked_duration_since(tokio::time::Instant::now())
                .unwrap_or_default();
            if remaining.is_zero() {
                return Err(format!("timed out waiting for message body '{needle}'"));
            }
            let _ = tokio::time::timeout(remaining, self.runtime.inbox_notify().notified()).await;
        }
    }
}

/// Send one plain peer message from `from` to `to` and return the typed
/// receipt.
///
/// Load-bearing fact making `Acked` or in-process `HandedOff` a
/// delivery-admission PROOF (DEC-P4H-9): the receiving io task sends a stream
/// transport ack only on `AdmissionOutcome::Admitted`, while the wait-based
/// in-process lane reports direct inbox handoff.
/// A sender with no trust row for `to` is refused by its own router with an
/// immediate typed error (trust-before-send).
pub async fn send_peer_text(
    from: &Arc<dyn meerkat_core::agent::CommsRuntime>,
    to: meerkat_core::comms::PeerId,
    body: &str,
) -> Result<SendReceipt, meerkat_core::SendError> {
    from.send(CommsCommand::PeerMessage {
        to: meerkat_core::PeerRoute::new(to),
        body: body.to_string(),
        blocks: None,
        content_taint: None,
        handling_mode: HandlingMode::Queue,
        objective_id: None,
    })
    .await
}

/// Craft an honest `BindHost` payload from a sender endpoint against a host
/// descriptor (raw-probe flavor of the W3.3 send; tests doctor individual
/// fields for the mismatch rows).
pub fn raw_bind_host_command(
    sender: &PeerCommsEndpoint,
    mob_id: &str,
    descriptor: &WireHostBindingDescriptor,
    epoch: u64,
) -> BridgeCommand {
    raw_bind_host_command_for_generation(sender, mob_id, descriptor, epoch, 1)
}

/// Craft an honest `BindHost` payload with an explicit host-binding
/// generation. Replacement binds after revoke must advance the durable
/// generation high-water mark instead of reusing generation one.
pub fn raw_bind_host_command_for_generation(
    sender: &PeerCommsEndpoint,
    mob_id: &str,
    descriptor: &WireHostBindingDescriptor,
    epoch: u64,
    binding_generation: u64,
) -> BridgeCommand {
    let resolved = descriptor
        .identity
        .resolve()
        .expect("descriptor identity resolves");
    let payload = meerkat_mob::runtime::bridge_protocol::BridgeHostBindPayload {
        supervisor: supervisor_spec_for_endpoint(sender, mob_id),
        epoch,
        binding_generation,
        protocol_version: BridgeProtocolVersion::V4,
        expected_host_peer_id: resolved.peer_id.to_string(),
        expected_address: descriptor.address.clone(),
        bootstrap_proof: meerkat_mob::runtime::bridge_protocol::BridgeHostBootstrapProof::new(""),
        mob_id: mob_id.to_string(),
        required_capabilities: Default::default(),
    };
    BridgeCommand::BindHost(
        meerkat_mob::runtime::bridge_protocol::seal_host_bind_bootstrap_proof(
            payload,
            &descriptor.bootstrap_token,
        ),
    )
}

/// Honest supervisor spec for a raw sender endpoint (the
/// `{mob_id}/__mob_supervisor__` display-name convention; identity comes from
/// the pubkey, never the name).
pub fn supervisor_spec_for_endpoint(sender: &PeerCommsEndpoint, mob_id: &str) -> BridgePeerSpec {
    BridgePeerSpec {
        name: format!("{mob_id}/__mob_supervisor__"),
        peer_id: sender.runtime.public_key().to_peer_id().to_string(),
        address: sender.runtime.advertised_address(),
        pubkey: *sender.runtime.public_key().as_bytes(),
    }
}

// ===========================================================================
// Phase-3 raw V4 command builders (host-addressed + member-originated)
// ===========================================================================

/// Raw `ReleaseMember` at an explicit idempotency tuple (the fence-matrix and
/// orphan-reconciliation probe flavor; tests doctor the tuple).
pub fn raw_release_member_command(
    sender: &PeerCommsEndpoint,
    mob_id: &str,
    epoch: u64,
    agent_identity: &str,
    generation: u64,
    fence_token: u64,
) -> BridgeCommand {
    BridgeCommand::ReleaseMember(BridgeReleasePayload {
        supervisor: supervisor_spec_for_endpoint(sender, mob_id),
        epoch,
        binding_generation: 1,
        protocol_version: BridgeProtocolVersion::V4,
        mob_id: mob_id.to_string(),
        agent_identity: agent_identity.to_string(),
        generation,
        fence_token,
    })
}

/// Raw `HostStatus` query for one mob's materialized inventory.
pub fn raw_host_status_command(
    sender: &PeerCommsEndpoint,
    mob_id: &str,
    epoch: u64,
) -> BridgeCommand {
    BridgeCommand::HostStatus(BridgeHostStatusPayload {
        supervisor: supervisor_spec_for_endpoint(sender, mob_id),
        epoch,
        binding_generation: 1,
        protocol_version: BridgeProtocolVersion::V4,
        mob_id: mob_id.to_string(),
    })
}

/// Raw member-originated operator request at the initial binding tuple.
/// Deliberately carries NO supervisor/epoch envelope and NO authority claim;
/// admission is MobMachine-owned. Use [`raw_member_operator_command_at`] for
/// stale-generation/fence probes.
pub fn raw_member_operator_command(
    agent_identity: &str,
    request_id: &str,
    op: MemberOperatorOp,
) -> BridgeCommand {
    raw_member_operator_command_at(
        MemberOperatorRequesterResidency {
            agent_identity,
            generation: 0,
            fence_token: 1,
            host_id: "host-a",
            host_binding_generation: 1,
            member_session_id: "unknown-member-session",
        },
        request_id,
        op,
    )
}

/// Exact requester residency carried by a raw member-operator command.
#[derive(Debug, Clone, Copy)]
pub struct MemberOperatorRequesterResidency<'a> {
    pub agent_identity: &'a str,
    pub generation: u64,
    pub fence_token: u64,
    pub host_id: &'a str,
    pub host_binding_generation: u64,
    pub member_session_id: &'a str,
}

/// Raw member-originated operator request at an explicit requester residency
/// tuple. Host identity/generation, member session, generation, and fence are
/// signed as part of the bridge command envelope and must all match the
/// controlling machine's current binding exactly.
pub fn raw_member_operator_command_at(
    requester: MemberOperatorRequesterResidency<'_>,
    request_id: &str,
    op: MemberOperatorOp,
) -> BridgeCommand {
    BridgeCommand::MemberOperatorRequest(BridgeMemberOperatorPayload {
        agent_identity: requester.agent_identity.to_string(),
        requester_generation: requester.generation,
        requester_fence_token: requester.fence_token,
        requester_host_id: requester.host_id.to_string(),
        requester_host_binding_generation: requester.host_binding_generation,
        requester_member_session_id: requester.member_session_id.to_string(),
        request_id: request_id.to_string(),
        op,
        protocol_version: BridgeProtocolVersion::V4,
    })
}

// ===========================================================================
// Phase-6 raw V4 command builders (member-addressed events/history/flow
// family; design-flow-spine §5.0). Epoch 1 = the bind_then_materialize
// ceremony epoch; tests doctor fields for negative rows.
// ===========================================================================

/// Raw `DeliverMemberInput`, optionally directive-bearing (§18 O1). Drives
/// member-side tracked-turn admission DIRECTLY, bypassing the controlling
/// actor's `ClassifyFlowStepDispatch` gate — the fail-closed backstop rows.
pub fn raw_deliver_member_input_command(
    sender: &PeerCommsEndpoint,
    mob_id: &str,
    input_id: &str,
    content: &str,
    expected_member: BridgeMemberIncarnation,
    turn: Option<BridgeTurnDirective>,
) -> BridgeCommand {
    BridgeCommand::DeliverMemberInput(BridgeDeliveryPayload {
        supervisor: supervisor_spec_for_endpoint(sender, mob_id),
        epoch: 1,
        protocol_version: BridgeProtocolVersion::V4,
        input_id: input_id.to_string(),
        transcript_interaction_id: None,
        content: content.into(),
        handling_mode: HandlingMode::Queue,
        objective_id: None,
        expected_member: Some(expected_member),
        injected_context: Vec::new(),
        turn,
        outcome_tracking: None,
    })
}

/// Raw `PollMemberEvents` at an explicit `(generation, seq)` cursor (or
/// `Tail`). `wait_ms: Some(0)`/`None` is the fast path; a bounded value
/// long-polls server-side.
pub fn raw_poll_member_events_command(
    sender: &PeerCommsEndpoint,
    mob_id: &str,
    epoch: u64,
    expected_member: BridgeMemberIncarnation,
    cursor: BridgeEventCursor,
    max: Option<u32>,
    wait_ms: Option<u32>,
) -> BridgeCommand {
    raw_poll_member_events_command_with_outcome_protocol(
        sender,
        mob_id,
        epoch,
        expected_member,
        cursor,
        max,
        wait_ms,
        Vec::new(),
        None,
    )
}

/// Raw poll with explicit independently acknowledged outcome controls.
#[allow(clippy::too_many_arguments)]
pub fn raw_poll_member_events_command_with_outcome_protocol(
    sender: &PeerCommsEndpoint,
    mob_id: &str,
    epoch: u64,
    expected_member: BridgeMemberIncarnation,
    cursor: BridgeEventCursor,
    max: Option<u32>,
    wait_ms: Option<u32>,
    outcome_acks: Vec<BridgeTurnOutcomeAck>,
    max_outcomes: Option<u32>,
) -> BridgeCommand {
    BridgeCommand::PollMemberEvents(BridgePollEventsPayload {
        supervisor: supervisor_spec_for_endpoint(sender, mob_id),
        epoch,
        protocol_version: BridgeProtocolVersion::V4,
        expected_member,
        cursor,
        max,
        outcome_acks,
        max_outcomes,
        wait_ms,
    })
}

/// Raw `ReadMemberHistory`. `from_index: None, limit: Some(n)` is the
/// tail-addressed single-round-trip page (DEC-P6E-6).
pub fn raw_read_member_history_command(
    sender: &PeerCommsEndpoint,
    mob_id: &str,
    epoch: u64,
    expected_member: BridgeMemberIncarnation,
    from_index: Option<u64>,
    limit: Option<u32>,
) -> BridgeCommand {
    BridgeCommand::ReadMemberHistory(BridgeReadHistoryPayload {
        supervisor: supervisor_spec_for_endpoint(sender, mob_id),
        epoch,
        protocol_version: BridgeProtocolVersion::V4,
        expected_member,
        from_index,
        limit,
    })
}

/// Raw `HardCancelMember` (the explicit non-cooperative cancel verb;
/// deliberately separate from `InterruptMember`).
pub fn raw_hard_cancel_member_command(
    sender: &PeerCommsEndpoint,
    mob_id: &str,
    epoch: u64,
    expected_member: BridgeMemberIncarnation,
    operation_id: meerkat_core::ops::OperationId,
    expected_run_id: meerkat_core::RunId,
    reason: &str,
) -> BridgeCommand {
    BridgeCommand::HardCancelMember(BridgeHardCancelPayload {
        supervisor: supervisor_spec_for_endpoint(sender, mob_id),
        epoch,
        protocol_version: BridgeProtocolVersion::V4,
        expected_member,
        operation_id,
        expected_run_id,
        reason: reason.to_string(),
    })
}

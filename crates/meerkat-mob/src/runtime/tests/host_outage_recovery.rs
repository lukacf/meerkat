//! A current outage invalidates cached boot evidence before an old reply can
//! publish recovery. Machine route intent still owns every trust install.

use super::placement_support as support;
use super::*;
use crate::SpawnMemberSpec;
use crate::machines::mob_machine as dsl;
use crate::runtime::bridge_protocol::{BridgeMemberIncarnation, BridgeRejectionCause};
use crate::runtime::scope_gate::RoutedMobCommand;
use crate::runtime::state::MobCommand;
use meerkat_contracts::wire::WireReachability;

// Queue each completion/observation/read batch while the actor is parked.
// The trailing park keeps periodic status retries from changing the asserted
// state after its serialized read, without changing production poll timing.
struct ParkedActor {
    release: tokio::sync::oneshot::Sender<()>,
    completed: tokio::sync::oneshot::Receiver<Result<(), MobError>>,
}

impl ParkedActor {
    async fn enqueue(handle: &MobHandle) -> (Self, tokio::sync::oneshot::Receiver<()>) {
        let (entered_tx, entered) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        let (reply_tx, completed) = tokio::sync::oneshot::channel();
        handle
            .command_tx
            .send(RoutedMobCommand::internal(
                MobCommand::ParkActorForObservationTest {
                    entered_tx,
                    release_rx,
                    reply_tx,
                },
            ))
            .await
            .expect("queue actor observation barrier");
        (Self { release, completed }, entered)
    }

    async fn enter(handle: &MobHandle) -> Self {
        let (parked, entered) = Self::enqueue(handle).await;
        entered.await.expect("actor entered observation barrier");
        parked
    }

    async fn release(self) {
        self.release.send(()).expect("release observation barrier");
        self.completed
            .await
            .expect("barrier reply")
            .expect("barrier released");
    }
}

struct RecoveryFixture {
    controlling: support::ControllingMob,
    host: support::ScriptedHostPeer,
    receiver: Arc<support::PeerCommsEndpoint>,
    sender: Arc<dyn CoreCommsRuntime>,
    expected: BridgeMemberIncarnation,
    epoch: u64,
}

impl RecoveryFixture {
    async fn new(label: &str) -> Self {
        let host = support::spawn_scripted_host_peer(label).await;
        let receiver = Arc::new(
            support::spawn_peer_comms_endpoint(&format!("{label}-remote"), true, None).await,
        );
        host.script_member_identity("remote", support::member_identity_of(&receiver));
        host.bind_member_endpoint("remote", Arc::clone(&receiver));
        let controlling = support::create_controlling_mob(label).await;
        let report = controlling.bind_scripted(&host).await;
        // The tests explicitly supply current status completions. Do not let
        // a background status response independently retry the held scenario.
        host.drop_next_host_status_replies(u64::MAX);
        controlling
            .handle
            .spawn_spec(SpawnMemberSpec::new("worker", "local"))
            .await
            .expect("local member");
        controlling
            .spawn_placed("worker", "remote", &report.host_id)
            .await
            .expect("placed receiver");
        controlling
            .handle
            .wire(AgentIdentity::from("local"), AgentIdentity::from("remote"))
            .await
            .expect("wire committed and installed");
        let state = controlling
            .handle
            .query_machine_state()
            .await
            .expect("machine state");
        let member = dsl::AgentIdentity::from("remote");
        let host_id = dsl::HostId::from(report.host_id.as_str());
        let expected = BridgeMemberIncarnation {
            mob_id: controlling.mob_id.to_string(),
            agent_identity: "remote".to_string(),
            host_id: report.host_id,
            binding_generation: state.host_binding_generations[&host_id],
            member_session_id: state.member_session_bindings[&member].0.clone(),
            generation: state.identity_runtime_generations[&member].0,
            fence_token: state.identity_runtime_fence_tokens[&member].0,
        };
        assert!(
            state.pending_route_installs.is_empty(),
            "initial wire converged"
        );
        let session = controlling
            .member_session_id(&AgentIdentity::from("local"))
            .await;
        let sender = controlling.member_comms_runtime(&session).await;
        Self {
            controlling,
            host,
            receiver,
            sender,
            expected,
            epoch: report.epoch,
        }
    }

    fn failure(&self, binding_incarnation: u64) -> MobCommand {
        MobCommand::HostStatusPollCompleted {
            host_id: self.expected.host_id.clone(),
            binding_epoch: self.epoch,
            binding_generation: self.expected.binding_generation,
            binding_incarnation,
            result: Err(MobError::BridgeCommandRejected {
                cause: BridgeRejectionCause::Unavailable,
                reason: "observed host outage".to_string(),
            }),
        }
    }

    async fn observe_same_token(
        &self,
        parked: ParkedActor,
        failure: Option<MobCommand>,
    ) -> (ParkedActor, dsl::MobMachineState) {
        let handle = &self.controlling.handle;
        if let Some(failure) = failure {
            handle
                .command_tx
                .send(RoutedMobCommand::internal(failure))
                .await
                .expect("queue status failure");
        }
        let (reply_tx, observed) = tokio::sync::oneshot::channel();
        handle
            .command_tx
            .send(RoutedMobCommand::internal(
                MobCommand::HostRuntimeIncarnationObserved {
                    expected_member: self.expected.clone(),
                    runtime_incarnation: self.host.runtime_incarnation,
                    reply_tx,
                },
            ))
            .await
            .expect("queue authenticated old-token observation");
        let (reply_tx, snapshot) = tokio::sync::oneshot::channel();
        handle
            .command_tx
            .send(RoutedMobCommand::internal(MobCommand::QueryMachineState {
                reply_tx,
            }))
            .await
            .expect("queue serialized route projection");
        let (next, entered) = ParkedActor::enqueue(handle).await;
        parked.release().await;
        observed
            .await
            .expect("runtime observation reply")
            .expect("transient route rejection stays pending without failing observation");
        let state = snapshot.await.expect("serialized route projection");
        entered.await.expect("hold post-observation projection");
        (next, state)
    }

    async fn receiver_trusts_sender(&self) -> bool {
        let sender_id = self.sender.peer_id().expect("sender peer id");
        self.receiver
            .runtime
            .public_trusted_peer_projection_snapshot()
            .await
            .expect("receiver trust projection")
            .iter()
            .any(|peer| peer.peer_id == sender_id)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_boot_token_after_current_status_failure_reinstalls_routes_before_recovery() {
    let _serial = lock_real_comms_tests();
    let fixture = RecoveryFixture::new("same-boot-outage").await;
    let parked = ParkedActor::enter(&fixture.controlling.handle).await;
    let (parked, initial) = fixture.observe_same_token(parked, None).await;
    assert!(initial.pending_route_installs.is_empty());
    assert!(
        fixture.receiver_trusts_sender().await,
        "initial real receiver trust"
    );
    let installs_before = fixture.host.install_peer_trust_count();
    let sender_id = fixture
        .sender
        .peer_id()
        .expect("sender peer id")
        .to_string();
    fixture.receiver.untrust(&sender_id).await;
    assert!(
        !fixture.receiver_trusts_sender().await,
        "model lost process-local trust"
    );
    fixture.host.reject_next_install_peer_trust(
        BridgeRejectionCause::Unavailable,
        "replacement host is not ready for trust",
    );

    // This fixture has one committed bind and no revoke/rebind. The current
    // process-local binding incarnation is therefore 1, independent of the
    // durable binding generation and supervisor epoch read above.
    let (parked, rejected) = fixture
        .observe_same_token(parked, Some(fixture.failure(1)))
        .await;
    assert_eq!(
        fixture.host.install_peer_trust_count(),
        installs_before + 1,
        "same-token observation after a current outage must reinstall machine routes"
    );
    assert_eq!(
        rejected.pending_route_installs.len(),
        1,
        "a rejected install must keep public route completeness false"
    );
    let pending = rejected
        .pending_route_installs
        .iter()
        .next()
        .expect("pending route");
    assert_eq!(pending.host.as_str(), fixture.expected.host_id);
    assert_eq!(
        (&pending.edge.a, &pending.edge.b),
        (
            &dsl::AgentIdentity::from("local"),
            &dsl::AgentIdentity::from("remote")
        )
    );
    assert!(
        !fixture.receiver_trusts_sender().await,
        "rejection cannot publish receiver trust"
    );
    let payloads = fixture.host.received_install_peer_trust_payloads();
    assert_eq!(
        payloads.last().expect("replayed install").peer.peer_id,
        sender_id
    );

    let (parked, recovered) = fixture.observe_same_token(parked, None).await;
    assert_eq!(
        fixture.host.install_peer_trust_count(),
        installs_before + 2,
        "ordinary next observation retries the outstanding install"
    );
    assert!(
        recovered.pending_route_installs.is_empty(),
        "ACK drains the ledger"
    );
    assert!(
        fixture.receiver_trusts_sender().await,
        "ACK follows actual receiver trust install"
    );
    let receipt = support::send_peer_text(
        &fixture.sender,
        fixture.receiver.self_descriptor().peer_id,
        "same-boot-recovered",
    )
    .await
    .expect("recovered route admits a real send");
    let admitted = match receipt {
        SendReceipt::PeerMessageSent {
            delivery: meerkat_core::comms::PeerDeliveryOutcome::Acked,
            ..
        } => true,
        SendReceipt::PeerMessageSent {
            delivery: meerkat_core::comms::PeerDeliveryOutcome::DurablyResolved { outcome },
            ..
        } => outcome.is_durable_admission(),
        _ => false,
    };
    assert!(admitted, "real receiver acknowledged admission");
    fixture
        .receiver
        .wait_for_message_body("same-boot-recovered", Duration::from_secs(5))
        .await
        .expect("receiver observed recovered message");
    parked.release().await;
    fixture.host.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_binding_status_failure_preserves_same_token_route_convergence() {
    let _serial = lock_real_comms_tests();
    let fixture = RecoveryFixture::new("stale-binding-outage").await;
    let parked = ParkedActor::enter(&fixture.controlling.handle).await;
    let (parked, _) = fixture.observe_same_token(parked, None).await;
    let installs_before = fixture.host.install_peer_trust_count();
    let handle = &fixture.controlling.handle;
    let before = handle
        .reachability_observations
        .host(&fixture.expected.host_id)
        .expect("host was observed");
    assert_eq!(before.reachability, WireReachability::Reachable);

    // Fence zero predates this fixture's first bind (incarnation one). Keep
    // the durable epoch and generation current so only this fence rejects it.
    handle
        .command_tx
        .send(RoutedMobCommand::internal(fixture.failure(0)))
        .await
        .expect("queue stale completion");
    let (next, entered) = ParkedActor::enqueue(handle).await;
    parked.release().await;
    entered.await.expect("stale completion processed");
    let after = handle
        .reachability_observations
        .host(&fixture.expected.host_id)
        .expect("current host observation survives stale failure");
    assert_eq!(after.reachability, before.reachability);
    assert_eq!(after.freshness_reason, before.freshness_reason);
    let (parked, state) = fixture.observe_same_token(next, None).await;
    assert_eq!(
        fixture.host.install_peer_trust_count(),
        installs_before,
        "stale failure must not invalidate current boot evidence or reinstall"
    );
    assert!(state.pending_route_installs.is_empty());
    assert!(fixture.receiver_trusts_sender().await);
    parked.release().await;
    fixture.host.shutdown();
}
